use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use kcp::Kcp;

use rumqttc::{AsyncClient, MqttOptions, QoS};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::select;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::{task, time};
use tracing::{debug, error, info, trace, warn};

use crate::topic;

const MAX_BUFFER_SIZE: usize = 4096;
const MQTT_BUFFER_CAPACITY: usize = 10;

pub struct MqttConfig {
    pub id: String,
    pub host: String,
    pub port: u16,
}

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

async fn up_kcp_vnet(
    mut socket: TcpStream,
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    mut receiver: UnboundedReceiver<(String, Vec<u8>)>,
) {
    let mut buf = [0; MAX_BUFFER_SIZE];

    let mut kcp = Kcp::new(0, ChannelOutput::new(key.clone(), sender.clone()));
    let mut interval = tokio::time::interval(time::Duration::from_millis(10));
    loop {
        select! {
            _ = interval.tick() => {
                kcp.update(now_millis()).unwrap();
            }
            Some((_key, mut raw)) = receiver.recv() => {
                kcp.input(raw.as_mut_slice()).unwrap();
                let n = match kcp.recv(buf.as_mut_slice()) {
                    Ok(n) => n,
                    Err(kcp::Error::RecvQueueEmpty) => continue,
                    Err(err) => panic!("kcp.recv error: {:?}", err),
                };
                socket.write_all(&buf[..n]).await
                .unwrap_or_else(|e| panic!("tcp vnet: {} write failed, error: {}", key, e));
            }
            Ok(n) = socket.read(&mut buf) => {
                if n == 0 { break };
                trace!("read {} bytes: {:?}", n, buf[..n].to_vec());
                kcp.send(&buf[..n])
                .unwrap_or_else(|e| panic!("tcp vnet: {} read failed, error: {}", key, e));
            }
            else => {
                debug!("Receiver channel closed or other case");
                break;
            }
        }
    }
    warn!("tcp vnet {} exit", key);
}

async fn up_tcp_vnet(
    mut socket: TcpStream,
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    mut receiver: UnboundedReceiver<(String, Vec<u8>)>,
) {
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        select! {
            Some((_key, data)) = receiver.recv() => {
                socket.write_all(data.as_slice()).await
                .unwrap_or_else(|e| panic!("tcp vnet: {} write failed, error: {}", key, e));
            }
            Ok(n) = socket.read(&mut buf) => {
                sender.send((key.clone(),
                        buf[..n].to_vec()
                    ))
                .unwrap_or_else(|e| panic!("tcp vnet: {} read failed, error: {}", key, e));
            }
            else => {
                debug!("Receiver channel closed or other case");
                break;
            }
        }
    }
    warn!("tcp vnet {} exit", key);
}

async fn up_udp_vnet(
    socket: UdpSocket,
    key: String,
    sender: UnboundedSender<(String, Vec<u8>)>,
    mut receiver: UnboundedReceiver<(String, Vec<u8>)>,
) {
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        select! {
            Some((_key, data)) = receiver.recv() => {
                socket.send(data.as_slice()).await.unwrap();
            }
            Ok(n) = socket.recv(&mut buf) => {
                sender.send((key.clone(),
                        buf[..n].to_vec()
                    )).unwrap();
            }
        }
    }
}

pub async fn agent(mqtt_config: &MqttConfig, address: SocketAddr, prefix: &str, server_id: &str) {
    let mut senders: HashMap<String, UnboundedSender<(String, Vec<u8>)>> = HashMap::new();
    let (sender, mut receiver) = unbounded_channel::<(String, Vec<u8>)>();

    let (client, mut eventloop) = AsyncClient::new(
        MqttOptions::new(&mqtt_config.id, &mqtt_config.host, mqtt_config.port),
        MQTT_BUFFER_CAPACITY,
    );
    client
        .subscribe(
            topic::build_sub(prefix, server_id, topic::ANY, topic::label::I),
            QoS::AtMostOnce,
        )
        .await
        .unwrap();

    loop {
        let sender_clone = sender.clone();
        select! {
            Some((key, data)) = receiver.recv() => {
                let (prefix, server_id, client_id, _label, protocol, address) = topic::parse(&key);
                client.publish(topic::build(prefix, server_id, client_id, topic::label::O, protocol, address),
                    QoS::AtMostOnce,
                    false,
                    data
                ).await.unwrap();
            }
            Ok(notification) = eventloop.poll() => {
                if let Some(p) = mqtt_receive(notification) {
                    let topic = p.topic.clone();
                    let (_prefix, _server_id, _client_id, _label, protocol, _address) = topic::parse(&topic);
                    let sender = match senders.get(&p.topic) {
                        Some(sender) => sender,
                        None => {
                            let (vnet_tx, vnet_rx) = unbounded_channel::<(String, Vec<u8>)>();
                            let topic = p.topic.clone();
                            match protocol {
                                topic::protocol::KCP => {
                                    task::spawn(async move {
                                        let socket = TcpStream::connect(address).await.unwrap();
                                        up_kcp_vnet(socket, topic, sender_clone, vnet_rx).await;
                                    });
                                },
                                topic::protocol::TCP => {
                                    task::spawn(async move {
                                        let socket = TcpStream::connect(address).await.unwrap();
                                        up_tcp_vnet(socket, topic, sender_clone, vnet_rx).await;
                                    });
                                },
                                topic::protocol::UDP => {
                                    task::spawn(async move {
                                        let socket = UdpSocket::bind(
                                                SocketAddr::new(
                                                // "0.0.0.0:0"
                                                // "[::]:0"
                                                match address {
                                                    SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                                                    SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                                                }, 0)
                                            ).await.unwrap();
                                        socket.connect(address).await.unwrap();
                                        up_udp_vnet(socket, topic, sender_clone, vnet_rx).await;
                                    });
                                },
                                e => info!("unknown protocol {}", e)
                            };
                            senders.insert(p.topic.clone(), vnet_tx);
                            senders.get(&p.topic).unwrap()
                        },
                    };
                    sender.send((p.topic, p.payload.to_vec())).unwrap();
                }
            }
            else => {
                error!("vserver proxy error");
            }
        }
    }
}

pub async fn local(
    mqtt_config: &MqttConfig,
    address: SocketAddr,
    prefix: &str,
    server_id: &str,
    client_id: &str,
    tcp_over_kcp: bool,
) {
    let mut senders: HashMap<String, UnboundedSender<(String, Vec<u8>)>> = HashMap::new();
    let (sender, mut receiver) = unbounded_channel::<(String, Vec<u8>)>();

    let (client, mut eventloop) = AsyncClient::new(
        MqttOptions::new(&mqtt_config.id, &mqtt_config.host, mqtt_config.port),
        MQTT_BUFFER_CAPACITY,
    );
    client
        .subscribe(
            topic::build_sub(prefix, topic::ANY, client_id, topic::label::O),
            QoS::AtMostOnce,
        )
        .await
        .unwrap();

    let mut buf = [0; MAX_BUFFER_SIZE];
    let listener = TcpListener::bind(address).await.unwrap();
    let sock = UdpSocket::bind(address).await.unwrap();
    loop {
        let sender_clone = sender.clone();
        select! {
            // TCP Server
            Ok((socket, _)) = listener.accept() => {
                let (vnet_tx, vnet_rx) = unbounded_channel::<(String, Vec<u8>)>();

                let protocol = if tcp_over_kcp { topic::protocol::KCP } else { topic::protocol::TCP };

                let addr = socket.peer_addr().unwrap().to_string();
                let key_send = topic::build(prefix, server_id, client_id, topic::label::I, protocol, &addr);
                let key_recv = topic::build(prefix, server_id, client_id, topic::label::O, protocol, &addr);

                senders.insert(key_recv, vnet_tx);
                task::spawn(async move {
                    if tcp_over_kcp {
                        up_kcp_vnet(socket, key_send, sender_clone, vnet_rx).await;
                    } else {
                        up_tcp_vnet(socket, key_send, sender_clone, vnet_rx).await;
                    };
                });
            }

            // UDP Server
            Ok((len, addr)) = sock.recv_from(&mut buf) => {
                sender_clone.send((
                    topic::build(prefix, server_id, client_id, topic::label::I, topic::protocol::UDP, &addr.to_string()),
                    buf[..len].to_vec())).unwrap();
            }

            Some((key, data)) = receiver.recv() => {
            client.publish(
                    key,
                    QoS::AtMostOnce,
                    false,
                    data
                ).await.unwrap();
            }
            Ok(notification) = eventloop.poll() => {
                if let Some(p) = mqtt_receive(notification) {
                    let topic = p.topic.clone();
                    let (_prefix, _server_id, _client_id, _label, protocol, address) = topic::parse(&topic);
                    match protocol {
                        topic::protocol::KCP => {
                            senders.get(&p.topic).unwrap().send((p.topic, p.payload.to_vec())).unwrap();
                        },
                        topic::protocol::TCP => {
                            senders.get(&p.topic).unwrap().send((p.topic, p.payload.to_vec())).unwrap();
                        },
                        topic::protocol::UDP => {
                            let _ = sock.send_to(&p.payload, address).await.unwrap();
                        },
                        e => info!("unknown protocol {}", e)
                    }
                }
            }
            else => {
                error!("vclient proxy error");
            }
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