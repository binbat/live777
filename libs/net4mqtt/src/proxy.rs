use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use anyhow::{anyhow, Error, Result};
use kcp::Kcp;
use lru_time_cache::LruCache;
use rumqttc::{AsyncClient, EventLoop, QoS};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::select;
use tokio::sync::mpsc::{unbounded_channel, Sender, UnboundedReceiver, UnboundedSender};
use tokio::{task, time};
use tracing::{debug, error, info, trace, warn};
use url::Url;

use crate::topic;

const MAX_BUFFER_SIZE: usize = 4096;
const MQTT_BUFFER_CAPACITY: usize = 10;
const LRU_MAX_CAPACITY: usize = 128;
const LRU_TIME_TO_LIVE: time::Duration = time::Duration::from_secs(300);

struct ChannelOutput {
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
}

impl ChannelOutput {
    pub fn new(key: String, sender: UnboundedSender<(String, Vec<u8>)>) -> Self {
        Self { key, sender }
    }
}

impl std::io::Write for ChannelOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = buf.len();
        self.sender.send((self.key.clone(), buf.to_vec())).unwrap();
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

use std::time::{SystemTime, UNIX_EPOCH};

#[inline]
pub fn now_millis() -> u32 {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .expect("time went afterwards");
    since_the_epoch.as_millis() as u32
}

async fn up_kcp_vnet<T>(
    mut socket: T,
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    mut receiver: UnboundedReceiver<(String, Vec<u8>)>,
) -> Result<(), Error>
where
    T: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let mut buf = [0; MAX_BUFFER_SIZE];

    let mut kcp = Kcp::new(0, ChannelOutput::new(key.clone(), sender));
    let mut interval = time::interval(time::Duration::from_millis(10));
    loop {
        select! {
            _ = interval.tick() => { kcp.update(now_millis())?; }
            result = receiver.recv() => {
                match result {
                    Some((_key, mut raw)) => {
                        kcp.input(raw.as_mut_slice())?;
                        match kcp.recv(buf.as_mut_slice()) {
                            Ok(n) => socket.write_all(&buf[..n]).await?,
                            Err(kcp::Error::RecvQueueEmpty) => continue,
                            Err(err) => return Err(anyhow!("kcp.recv error: {:?}", err)),
                        };
                    }
                    None => return Err(anyhow!("receiver is None")),
                }
            }
            result = socket.read(&mut buf) => {
                match result {
                    Ok(n) => {
                        trace!("read {} bytes: {:?}", n, buf[..n].to_vec());
                        if n == 0 { break };
                        kcp.send(&buf[..n])?;
                    }
                    Err(e) => return Err(anyhow!(e)),
                }
            }
            else => { break; }
        }
    }
    warn!("kcp vnet {} exit", &key);
    Ok(())
}

async fn up_tcp_vnet<T>(
    mut socket: T,
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    mut receiver: UnboundedReceiver<(String, Vec<u8>)>,
) -> Result<(), Error>
where
    T: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        select! {
            result = receiver.recv() => {
                match result {
                    Some((_key, data)) => { socket.write_all(data.as_slice()).await?; }
                    None => return Err(anyhow!("receiver is None")),
                }
            }
            result = socket.read(&mut buf) => {
                match result {
                    Ok(n) => {
                        trace!("read {} bytes: {:?}", n, buf[..n].to_vec());
                        if n == 0 { break };
                        sender.send((key.clone(),
                            buf[..n].to_vec()
                        ))?
                    }
                    Err(e) => return Err(anyhow!(e)),
                }
            }
            else => { break; }
        }
    }
    warn!("tcp vnet {} exit", key);
    Ok(())
}

async fn up_udp_vnet(
    socket: UdpSocket,
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    mut receiver: UnboundedReceiver<(String, Vec<u8>)>,
) -> Result<(), Error> {
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        select! {
            result = receiver.recv() => {
                match result {
                    Some((_key, data)) => { socket.send(data.as_slice()).await?; }
                    None => return Err(anyhow!("receiver is None")),
                }
            }
            result = socket.recv(&mut buf) => {
                match result {
                    Ok(n) => {
                        trace!("read {} bytes: {:?}", n, buf[..n].to_vec());
                        if n == 0 { continue };
                        sender.send((key.clone(),
                            buf[..n].to_vec()
                        ))?;
                    }
                    Err(e) => return Err(anyhow!(e)),
                }
            }
            else => { break; }
        }
    }
    warn!("udp vnet {} exit", key);
    Ok(())
}

async fn up_agent_vclient(
    address: &str,
    protocol: &str,
    topic: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    receiver: UnboundedReceiver<(String, Vec<u8>)>,
) -> Result<(), Error> {
    match protocol {
        topic::protocol::KCP => {
            let socket = TcpStream::connect(address).await?;
            up_kcp_vnet(socket, topic, sender, receiver).await
        }
        topic::protocol::TCP => {
            let socket = TcpStream::connect(address).await?;
            up_tcp_vnet(socket, topic, sender, receiver).await
        }
        topic::protocol::UDP => {
            let socket = UdpSocket::bind(SocketAddr::new(
                // "0.0.0.0:0"
                // "[::]:0"
                match SocketAddr::from_str(address)? {
                    SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                },
                0,
            ))
            .await?;
            socket.connect(address).await?;
            up_udp_vnet(socket, topic, sender, receiver).await
        }
        e => Err(anyhow!("unknown protocol {}", e)),
    }
}

async fn mqtt_client_init(
    mqtt_url: &str,
    topic_io_sub: (&str, &str, &str),
    id: (&str, &str),
    vdata: Option<VDataConfig>,
    xdata: Option<XDataConfig>,
) -> Result<
    (
        AsyncClient,
        EventLoop,
        String,
        Option<Sender<(String, String, Vec<u8>)>>,
        Option<UnboundedSender<(String, Vec<u8>)>>,
        UnboundedReceiver<(String, Vec<u8>)>,
    ),
    Error,
> {
    let (agent_id, local_id) = id;
    let (url, prefix) = crate::utils::pre_url(mqtt_url.parse::<Url>()?);

    let (x_sender, x_receiver) = {
        let XDataConfig { sender, receiver } = xdata.unwrap_or_default();
        (
            sender,
            receiver.unwrap_or_else(|| {
                let (sender, receiver) = unbounded_channel::<(String, Vec<u8>)>();
                std::mem::forget(sender);
                receiver
            }),
        )
    };

    let VDataConfig {
        online,
        offline,
        receiver,
    } = vdata.unwrap_or_default();
    let topic_v_sub = topic::build_sub(&prefix, topic::ANY, topic::ANY, topic::label::V);
    let topic_v_pub = topic::build_pub_x(&prefix, agent_id, local_id, topic::label::V, topic::NIL);

    let mut mqtt_options = rumqttc::MqttOptions::parse_url(url)?;
    debug!("mqtt_options: {:?}", mqtt_options);

    // MQTT LastWill
    // MQTT Client OnDisconnected publish at label::V
    // NOTE:
    // MQTT Payload data is null, retain will loss
    if let Some(v) = offline {
        mqtt_options.set_last_will(rumqttc::mqttbytes::v4::LastWill {
            topic: topic_v_pub.clone(),
            qos: QoS::AtMostOnce,
            retain: true,
            message: v.into(),
        });
    }

    let (a, b, c) = topic_io_sub;
    let (client, eventloop) = AsyncClient::new(mqtt_options, MQTT_BUFFER_CAPACITY);
    client
        .subscribe(topic::build_sub(&prefix, a, b, c), QoS::AtMostOnce)
        .await
        .unwrap();

    // MQTT Client OnConnected publish at label::V
    if let Some(v) = online {
        client
            .publish(topic_v_pub, QoS::AtMostOnce, true, v)
            .await
            .unwrap();
    }

    // MQTT subscribe at label::V
    if receiver.is_some() {
        client
            .subscribe(topic_v_sub, QoS::AtMostOnce)
            .await
            .unwrap();
    }

    // MQTT subscribe at label::X
    if x_sender.is_some() {
        client
            .subscribe(
                topic::build_sub(&prefix, topic::ANY, topic::ANY, topic::label::X),
                QoS::AtMostOnce,
            )
            .await
            .unwrap();
    }
    Ok((client, eventloop, prefix, receiver, x_sender, x_receiver))
}

#[derive(Default)]
pub struct VDataConfig {
    pub online: Option<Vec<u8>>,
    pub offline: Option<Vec<u8>>,
    pub receiver: Option<Sender<(String, String, Vec<u8>)>>,
}

#[derive(Default)]
pub struct XDataConfig {
    pub sender: Option<UnboundedSender<(String, Vec<u8>)>>,
    pub receiver: Option<UnboundedReceiver<(String, Vec<u8>)>>,
}

/// Agent service
///
/// # Arguments
///
/// * `mqtt_url` - The name of the person to greet, as a string slice.
/// * `address` - The age of the default target address, if local no set `DST`, use this as address.
/// * `agent_id` - The ID is all agents unique ID.
/// * `vdata` - MQTT system message: (online, offline, on_receiver(online, offline))
/// * `xdata` - User message: (sender, receiver)
///
/// # Examples
///
/// ```
/// net4mqtt::proxy::agent("mqtt://127.0.0.1:1883", "127.0.0.1:4444", "agent-0", None, None);
/// ```
pub async fn agent(
    mqtt_url: &str,
    address: &str,
    agent_id: &str,
    vdata: Option<VDataConfig>,
    xdata: Option<XDataConfig>,
) -> Result<(), Error> {
    let (client, mut eventloop, prefix, on_vdata, x_sender, mut x_receiver) = mqtt_client_init(
        mqtt_url,
        (agent_id, topic::ANY, topic::label::I),
        (agent_id, topic::NIL),
        vdata,
        xdata,
    )
    .await?;

    let mut senders =
        LruCache::<String, UnboundedSender<(String, Vec<u8>)>>::with_expiry_duration_and_capacity(
            LRU_TIME_TO_LIVE,
            LRU_MAX_CAPACITY,
        );

    let (sender, mut receiver) = unbounded_channel::<(String, Vec<u8>)>();
    loop {
        let sender = sender.clone();
        let x_sender = x_sender.clone();
        let on_vdata = on_vdata.clone();
        select! {
            result = x_receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(topic::build_pub_x(&prefix, agent_id, topic::NIL, topic::label::X, &key),
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        let (prefix, agent_id, local_id, _label, protocol, src, dst) = topic::parse(&key);
                        client.publish(topic::build(prefix, agent_id, local_id, topic::label::O, protocol, src, dst),
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = eventloop.poll() => {
                match result {
                    Ok(notification) => {
                        if let Some(p) = mqtt_receive(notification) {
                            let topic = p.topic.clone();
                            let (_prefix, agent_id, local_id, label, protocol, _src, dst) = topic::parse(&topic);

                            match label {
                                topic::label::V => {
                                    if let Some(s) = on_vdata {
                                        s.send((agent_id.to_string(), local_id.to_string(), p.payload.to_vec())).await?;
                                    }
                                },
                                topic::label::X => {
                                    if let Some(s) = x_sender {
                                        s.send((protocol.to_string(), p.payload.to_vec()))?;
                                    }
                                },
                                _ => {
                                    let sender = match senders.get(&p.topic) {
                                        Some(sender) => sender,
                                        None => {
                                            let (vnet_tx, vnet_rx) = unbounded_channel::<(String, Vec<u8>)>();
                                            let topic = p.topic.clone();
                                            let protocol = protocol.to_string();
                                            let dst = if dst == topic::NIL { address } else { dst }.to_string();
                                            task::spawn(async move {
                                                if let Err(e) = up_agent_vclient(&dst, &protocol, topic, sender, vnet_rx).await {
                                                    error!("agent vnet error: {:?}", e)
                                                }
                                            });
                                            senders.insert(p.topic.clone(), vnet_tx);
                                            senders.get(&p.topic).unwrap()
                                        },
                                    };
                                    if sender.is_closed() {
                                        senders.remove(&p.topic);
                                    } else {
                                        sender.send((p.topic, p.payload.to_vec()))?;
                                    }
                                },
                            }
                        }
                    },
                    Err(e) => {
                        error!("agent mqtt error: {:?}", e);
                        time::sleep(time::Duration::from_secs(1)).await;
                    }
                }
            }
            else => { error!("vagent proxy error"); }
        }
    }
}

pub async fn local_ports_tcp(
    mqtt_url: &str,
    listener: TcpListener,
    target: Option<String>,
    id: (&str, &str),
    vdata: Option<VDataConfig>,
    xdata: Option<XDataConfig>,
    tcp_over_kcp: bool,
) -> Result<(), Error> {
    let (agent_id, local_id) = id;

    let (client, mut eventloop, prefix, on_vdata, x_sender, mut x_receiver) = mqtt_client_init(
        mqtt_url,
        (topic::ANY, local_id, topic::label::O),
        (topic::NIL, local_id),
        vdata,
        xdata,
    )
    .await?;

    let target = target.unwrap_or(topic::NIL.to_string());
    let mut senders =
        LruCache::<String, UnboundedSender<(String, Vec<u8>)>>::with_expiry_duration_and_capacity(
            LRU_TIME_TO_LIVE,
            LRU_MAX_CAPACITY,
        );
    let (sender, mut receiver) = unbounded_channel::<(String, Vec<u8>)>();
    loop {
        let sender = sender.clone();
        let x_sender = x_sender.clone();
        let on_vdata = on_vdata.clone();
        select! {
            Ok((socket, _)) = listener.accept() => {
                let (vnet_tx, vnet_rx) = unbounded_channel::<(String, Vec<u8>)>();

                let protocol = if tcp_over_kcp { topic::protocol::KCP } else { topic::protocol::TCP };

                let addr = socket.peer_addr().unwrap().to_string();
                let key_send = topic::build(&prefix, agent_id, local_id, topic::label::I, protocol, &addr, &target);
                let key_recv = topic::build(&prefix, agent_id, local_id, topic::label::O, protocol, &addr, &target);

                senders.insert(key_recv, vnet_tx);
                task::spawn(async move {
                    if let Err(e) = if tcp_over_kcp {
                        up_kcp_vnet(socket, key_send, sender, vnet_rx).await
                    } else {
                        up_tcp_vnet(socket, key_send, sender, vnet_rx).await
                    } { error!("local vnet error: {}", e) };
                });
            }
            result = x_receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(topic::build_pub_x(&prefix, topic::NIL, local_id, topic::label::X, &key),
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(
                            key,
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = eventloop.poll() => {
                match result {
                    Ok(notification) => {
                        if let Some(p) = mqtt_receive(notification) {
                            let topic = p.topic.clone();
                            let (_prefix, agent_id, local_id, label, protocol, _src, _dst) = topic::parse(&topic);

                            match (label, protocol) {
                                (topic::label::V, _) => {
                                    if let Some(s) = on_vdata {
                                        s.send((agent_id.to_string(), local_id.to_string(), p.payload.to_vec())).await?;
                                    }
                                },
                                (topic::label::X, _) => {
                                    if let Some(s) = x_sender {
                                        s.send((protocol.to_string(), p.payload.to_vec()))?;
                                    }
                                },
                                (_, topic::protocol::KCP | topic::protocol::TCP) => {
                                    if let Some(sender) = senders.get(&p.topic) {
                                        if sender.is_closed() {
                                            senders.remove(&p.topic);
                                        } else {
                                            sender.send((p.topic, p.payload.to_vec()))?;
                                        }
                                    }
                                },
                                (label, protocol) => info!("unknown label: {} and protocol: {}", label, protocol)
                            }
                        }
                    },
                    Err(e) => {
                        error!("local mqtt error: {:?}", e);
                        time::sleep(time::Duration::from_secs(1)).await;
                    }
                }

            }
            else => { error!("vlocal proxy error"); }
        }
    }
}

pub async fn local_ports_udp(
    mqtt_url: &str,
    sock: UdpSocket,
    target: Option<String>,
    id: (&str, &str),
    vdata: Option<VDataConfig>,
    xdata: Option<XDataConfig>,
) -> Result<(), Error> {
    let (agent_id, local_id) = id;

    let (client, mut eventloop, prefix, on_vdata, x_sender, mut x_receiver) = mqtt_client_init(
        mqtt_url,
        (topic::ANY, local_id, topic::label::O),
        (topic::NIL, local_id),
        vdata,
        xdata,
    )
    .await?;

    let target = target.unwrap_or(topic::NIL.to_string());
    let mut buf = [0; MAX_BUFFER_SIZE];
    let (sender, mut receiver) = unbounded_channel::<(String, Vec<u8>)>();
    loop {
        let sender = sender.clone();
        let x_sender = x_sender.clone();
        let on_vdata = on_vdata.clone();
        select! {
            Ok((len, addr)) = sock.recv_from(&mut buf) => {
                sender.send((
                    topic::build(&prefix, agent_id, local_id, topic::label::I, topic::protocol::UDP, &addr.to_string(), &target),
                    buf[..len].to_vec())).unwrap();
            }
            result = x_receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(topic::build_pub_x(&prefix, topic::NIL, local_id, topic::label::X, &key),
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(
                            key,
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = eventloop.poll() => {
                match result {
                    Ok(notification) => {
                        if let Some(p) = mqtt_receive(notification) {
                            let topic = p.topic.clone();
                            let (_prefix, _agent_id, _local_id, label, protocol, src, _dst) = topic::parse(&topic);

                            match (label, protocol) {
                                (topic::label::V, _) => {
                                    if let Some(s) = on_vdata {
                                        s.send((agent_id.to_string(), local_id.to_string(), p.payload.to_vec())).await?;
                                    }
                                },
                                (topic::label::X, _) => {
                                    if let Some(s) = x_sender {
                                        s.send((protocol.to_string(), p.payload.to_vec()))?;
                                    }
                                },
                                (_, topic::protocol::UDP) => { let _ = sock.send_to(&p.payload, src).await?; },
                                (label, protocol) => info!("unknown label: {} and protocol: {}", label, protocol)
                            }
                        }
                    },
                    Err(e) => {
                        error!("local mqtt error: {:?}", e);
                        time::sleep(time::Duration::from_secs(1)).await;
                    }
                }

            }
            else => { error!("vlocal proxy error"); }
        }
    }
}

fn mqtt_receive(raw: rumqttc::Event) -> Option<rumqttc::mqttbytes::v4::Publish> {
    match raw {
        rumqttc::Event::Incoming(event) => match event {
            rumqttc::Incoming::Publish(p) => Some(p),
            _ => None,
        },
        rumqttc::Event::Outgoing(_) => None,
    }
}

use socks5_server::{auth::NoAuth, Server};

use std::sync::Arc;

pub async fn local_socks(
    mqtt_url: &str,
    listener: TcpListener,
    id: (&str, &str),
    domain: Option<String>,
    vdata: Option<VDataConfig>,
    xdata: Option<XDataConfig>,
    tcp_over_kcp: bool,
) -> Result<(), Error> {
    let (agent_id, local_id) = id;

    let (client, mut eventloop, prefix, on_vdata, x_sender, mut x_receiver) = mqtt_client_init(
        mqtt_url,
        (topic::ANY, local_id, topic::label::O),
        (topic::NIL, local_id),
        vdata,
        xdata,
    )
    .await?;

    let server = Server::new(listener, Arc::new(NoAuth));
    let mut senders =
        LruCache::<String, UnboundedSender<(String, Vec<u8>)>>::with_expiry_duration_and_capacity(
            LRU_TIME_TO_LIVE,
            LRU_MAX_CAPACITY,
        );
    let (sender, mut receiver) = unbounded_channel::<(String, Vec<u8>)>();
    loop {
        let sender = sender.clone();
        let x_sender = x_sender.clone();
        let on_vdata = on_vdata.clone();
        select! {
            Ok((conn, _)) = server.accept() => {
                match crate::socks::handle(conn, domain.clone()).await {
                    Ok((id, target, socket)) => {
                        let agent_id = match id {
                            Some(id) => id,
                            None => agent_id.to_string(),
                        };
                        let target = match target {
                            Some(t) => t,
                            None => topic::NIL.to_string(),
                        };

                        let (vnet_tx, vnet_rx) = unbounded_channel::<(String, Vec<u8>)>();

                        let protocol = if tcp_over_kcp { topic::protocol::KCP } else { topic::protocol::TCP };

                        let addr = socket.peer_addr().unwrap().to_string();
                        let key_send = topic::build(&prefix, &agent_id, local_id, topic::label::I, protocol, &addr, &target);
                        let key_recv = topic::build(&prefix, &agent_id, local_id, topic::label::O, protocol, &addr, &target);

                        senders.insert(key_recv, vnet_tx);
                        task::spawn(async move {
                            if let Err(e) = if tcp_over_kcp {
                                up_kcp_vnet(socket, key_send, sender, vnet_rx).await
                            } else {
                                up_tcp_vnet(socket, key_send, sender, vnet_rx).await
                            } { error!("local vnet error: {}", e) };
                        });

                    }
                    Err(err) => eprintln!("{err}"),
                }
            }

            result = x_receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(topic::build_pub_x(&prefix, topic::NIL, local_id, topic::label::X, &key),
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = receiver.recv() => {
                match result {
                    Some((key, data)) => {
                        client.publish(
                            key,
                            QoS::AtMostOnce,
                            false,
                            data
                        ).await?;
                    }
                    None => return Err(anyhow!("recv error"))
                }
            }
            result = eventloop.poll() => {
                match result {
                    Ok(notification) => {
                        if let Some(p) = mqtt_receive(notification) {
                            let topic = p.topic.clone();
                            let (_prefix, agent_id, local_id, label, protocol, _src, _dst) = topic::parse(&topic);

                            match (label, protocol) {
                                (topic::label::V, _) => {
                                    if let Some(s) = on_vdata {
                                        s.send((agent_id.to_string(), local_id.to_string(), p.payload.to_vec())).await.unwrap();
                                    }
                                },
                                (topic::label::X, _) => {
                                    if let Some(s) = x_sender {
                                        s.send((protocol.to_string(), p.payload.to_vec()))?;
                                    }
                                },
                                (_, topic::protocol::KCP | topic::protocol::TCP) => {
                                    if let Some(sender) = senders.get(&p.topic) {
                                        if sender.is_closed() {
                                            senders.remove(&p.topic);
                                        } else {
                                            sender.send((p.topic, p.payload.to_vec())).unwrap();
                                        }
                                    }
                                },
                                //topic::protocol::UDP => {
                                //    // TODO:
                                //    let _ = sock.send_to(&p.payload, address).await.unwrap();
                                //},
                                (label, protocol) => info!("unknown label: {} and protocol: {}", label, protocol)
                            }
                        }
                    },
                    Err(e) => {
                        error!("local mqtt error: {:?}", e);
                        time::sleep(time::Duration::from_secs(1)).await;
                    }
                }

            }
            else => { error!("vsocks proxy error"); }
        }
    }
}
