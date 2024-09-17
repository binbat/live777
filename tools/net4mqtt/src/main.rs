use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::thread;

use kcp::Kcp;

use rumqttc::{AsyncClient, MqttOptions, QoS};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::runtime::Runtime;
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::{task, time};
use tracing::{debug, error, info, trace, warn};

mod broker;
mod topic;

const MAX_BUFFER_SIZE: usize = 4096;
const MQTT_BUFFER_CAPACITY: usize = 10;

async fn check_port_availability(addr: SocketAddr) -> bool {
    TcpStream::connect(addr).await.is_ok()
}

async fn wait_for_port_availabilty(addr: SocketAddr) -> bool {
    let mut interval = tokio::time::interval(time::Duration::from_secs(1));
    loop {
        if check_port_availability(addr).await {
            return true;
        }
        interval.tick().await;
    }
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

async fn up_agent_proxy(
    mqtt_config: &MqttConfig,
    address: SocketAddr,
    prefix: &str,
    server_id: &str,
) {
    let mut senders: HashMap<String, UnboundedSender<(String, Vec<u8>)>> = HashMap::new();
    let (sender, mut receiver) = mpsc::unbounded_channel::<(String, Vec<u8>)>();

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
                match notification {
                    rumqttc::Event::Incoming(event) => {
                        match event {
                            rumqttc::Incoming::Publish(p) => {
                                let topic = p.topic.clone();
                                let (_prefix, _server_id, _client_id, _label, protocol, _address) = topic::parse(&topic);
                                let sender = match senders.get(&p.topic) {
                                    Some(sender) => sender,
                                    None => {
                                        let (vnet_tx, vnet_rx) = mpsc::unbounded_channel::<(String, Vec<u8>)>();
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
                                            e => panic!("unknown protocol {}", e)
                                        };
                                        senders.insert(p.topic.clone(), vnet_tx);
                                        senders.get(&p.topic).unwrap()
                                    },
                                };
                                sender.send((p.topic, p.payload.to_vec())).unwrap();
                            },
                            ev => info!("{:?}", ev)
                        }
                    }
                    rumqttc::Event::Outgoing(_) => {},
                }
            }
            else => {
                error!("vserver proxy error");
            }
        }
    }
}

async fn up_local_proxy(
    mqtt_config: &MqttConfig,
    address: SocketAddr,
    prefix: &str,
    server_id: &str,
    client_id: &str,
    tcp_over_kcp: bool,
) {
    let mut senders: HashMap<String, UnboundedSender<(String, Vec<u8>)>> = HashMap::new();
    let (sender, mut receiver) = mpsc::unbounded_channel::<(String, Vec<u8>)>();

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
                let (vnet_tx, vnet_rx) = mpsc::unbounded_channel::<(String, Vec<u8>)>();

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
                match notification {
                    rumqttc::Event::Incoming(event) => {
                        match event {
                            rumqttc::Incoming::Publish(p) => {
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
                                    _ => {},
                                }
                            },
                            ev => println!("{:?}", ev)
                        }
                    },
                    rumqttc::Event::Outgoing(_) => {},
                }
            }
            else => {
                error!("vclient proxy error");
            }
        }
    }
}

async fn up_echo_udp_server(listen: SocketAddr) {
    let sock = UdpSocket::bind(listen).await.unwrap();
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        let (n, addr) = sock.recv_from(&mut buf).await.unwrap();
        let _ = sock.send_to(&buf[..n], addr).await.unwrap();
    }
}

async fn up_echo_tcp_server(listen: SocketAddr) {
    let listener = TcpListener::bind(listen).await.unwrap();
    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let mut buf = [0; MAX_BUFFER_SIZE];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(_) => return,
                };
                if socket.write_all(&buf[0..n]).await.is_err() {
                    return;
                }
            }
        });
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mqtt_topic_prefix: &str = "test";
    let mqtt_broker_port: u16 = 1883;
    let agent_port: u16 = 4433;
    let local_port: u16 = 4444;
    let tcp_over_kcp: bool = true;

    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    //let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let mqtt_broker_host = ip;

    let agent_id = "0";
    let local_id = "0";
    let mqtt_id_proxy_agent = format!("test-proxy-agent-{}", agent_id);
    let mqtt_id_proxy_local = format!("test-proxy-local-{}", local_id);

    let agent_addr = SocketAddr::new(ip, agent_port);
    let local_addr = SocketAddr::new(ip, local_port);

    up_simulate_server(
        if args.len() == 2 {
            Some(SocketAddr::new(ip, agent_port))
        } else {
            None
        },
        Some(SocketAddr::new(ip, mqtt_broker_port)),
    )
    .await;

    thread::spawn(move || {
        Runtime::new().unwrap().block_on(async move {
            up_agent_proxy(
                &MqttConfig {
                    id: mqtt_id_proxy_agent,
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                agent_addr,
                mqtt_topic_prefix,
                agent_id,
            )
            .await
        })
    });

    thread::spawn(move || {
        Runtime::new().unwrap().block_on(async move {
            up_local_proxy(
                &MqttConfig {
                    id: mqtt_id_proxy_local,
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                local_addr,
                mqtt_topic_prefix,
                agent_id,
                local_id,
                tcp_over_kcp,
            )
            .await
        })
    });

    loop {
        time::sleep(time::Duration::from_millis(100)).await;
    }
}

async fn up_simulate_server(echo: Option<SocketAddr>, broker: Option<SocketAddr>) {
    if let Some(addr) = echo {
        thread::spawn(move || {
            Runtime::new()
                .unwrap()
                .block_on(async move { up_echo_tcp_server(addr).await })
        });

        thread::spawn(move || {
            Runtime::new()
                .unwrap()
                .block_on(async move { up_echo_udp_server(addr).await })
        });
    }

    if let Some(addr) = broker {
        thread::spawn(move || broker::up_mqtt_broker(addr));
        wait_for_port_availabilty(addr).await;
    }
}

pub struct MqttConfig {
    pub id: String,
    pub host: String,
    pub port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    use portpicker::pick_unused_port;

    const MQTT_TOPIC_PREFIX: &str = "test";
    const TCP_OVER_KCP: bool = true;

    async fn helper_up_proxy(ip: IpAddr) -> u16 {
        let mqtt_broker_host = ip;
        let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
        let agent_port: u16 = pick_unused_port().expect("No ports free");
        let local_port: u16 = pick_unused_port().expect("No ports free");

        let agent_id = "0";
        let local_id = "0";
        let mqtt_id_proxy_agent = format!("test-proxy-agent-{}", agent_id);
        let mqtt_id_proxy_local = format!("test-proxy-local-{}", local_id);

        let agent_addr = SocketAddr::new(ip, agent_port);
        let local_addr = SocketAddr::new(ip, local_port);

        up_simulate_server(
            Some(SocketAddr::new(ip, agent_port)),
            Some(SocketAddr::new(ip, mqtt_broker_port)),
        )
        .await;

        thread::spawn(move || {
            Runtime::new().unwrap().block_on(async move {
                up_agent_proxy(
                    &MqttConfig {
                        id: mqtt_id_proxy_agent,
                        host: mqtt_broker_host.to_string(),
                        port: mqtt_broker_port,
                    },
                    agent_addr,
                    MQTT_TOPIC_PREFIX,
                    agent_id,
                )
                .await
            })
        });

        thread::spawn(move || {
            Runtime::new().unwrap().block_on(async move {
                up_local_proxy(
                    &MqttConfig {
                        id: mqtt_id_proxy_local,
                        host: mqtt_broker_host.to_string(),
                        port: mqtt_broker_port,
                    },
                    local_addr,
                    MQTT_TOPIC_PREFIX,
                    agent_id,
                    local_id,
                    TCP_OVER_KCP,
                )
                .await
            })
        });
        local_port
    }

    #[tokio::test]
    async fn test_udp_simple() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let local_port = helper_up_proxy(ip).await;
        time::sleep(time::Duration::from_millis(10)).await;

        let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
        sock.connect(SocketAddr::new(ip, local_port)).await.unwrap();
        let mut buf = [0; MAX_BUFFER_SIZE];
        let test_msg = b"hello, world";
        sock.send(test_msg).await.unwrap();
        let len = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg);

        let test_msg2 = b"hello, world2";
        sock.send(test_msg2).await.unwrap();
        let len = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg2);
    }

    #[tokio::test]
    async fn test_udp_ipv6() {
        let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let local_port = helper_up_proxy(ip).await;
        time::sleep(time::Duration::from_millis(10)).await;

        let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
        sock.connect(SocketAddr::new(ip, local_port)).await.unwrap();
        let mut buf = [0; MAX_BUFFER_SIZE];
        let test_msg = b"hello, world";
        sock.send(test_msg).await.unwrap();
        let len = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg);

        let test_msg2 = b"hello, world2";
        sock.send(test_msg2).await.unwrap();
        let len = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg2);
    }

    #[tokio::test]
    async fn test_udp_two_connect() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let local_port = helper_up_proxy(ip).await;
        time::sleep(time::Duration::from_millis(10)).await;

        let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
        let sock2 = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
        sock.connect(SocketAddr::new(ip, local_port)).await.unwrap();
        sock2
            .connect(SocketAddr::new(ip, local_port))
            .await
            .unwrap();

        let mut buf = [0; MAX_BUFFER_SIZE];
        let test_msg = b"hello, world";
        let test_2_msg = b"hello, world 22222222222222222222";
        sock.send(test_msg).await.unwrap();
        sock2.send(test_2_msg).await.unwrap();
        let len = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg);
        let len = sock2.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_2_msg);

        let test_2_msg2 = b"hello, world yyyyyyyy";
        sock2.send(test_2_msg2).await.unwrap();
        let len = sock2.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_2_msg2);

        let test_msg2 = b"hello, world2";
        sock.send(test_msg2).await.unwrap();
        let len = sock.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg2);

        let test_2_msg3 = b"hello, world 333333";
        sock2.send(test_2_msg3).await.unwrap();
        let len = sock2.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_2_msg3);
    }

    #[tokio::test]
    async fn test_tcp() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let local_port = helper_up_proxy(ip).await;
        time::sleep(time::Duration::from_millis(10)).await;

        let mut socket = TcpStream::connect(SocketAddr::new(ip, local_port))
            .await
            .unwrap();

        let mut buf = [0; MAX_BUFFER_SIZE];
        let test_msg = b"hello, world";
        socket.write_all(test_msg).await.unwrap();
        let len = socket.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg);

        let test_msg2 = b"hello, world2";
        socket.write_all(test_msg2).await.unwrap();
        let len = socket.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg2);
    }
}
