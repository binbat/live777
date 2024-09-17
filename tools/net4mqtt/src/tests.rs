use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::thread;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::runtime::Runtime;
use tokio::time;

use crate::broker;

const MAX_BUFFER_SIZE: usize = 4096;

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

pub async fn up_simulate_server(echo: Option<SocketAddr>, broker: Option<SocketAddr>) {
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

use portpicker::pick_unused_port;

use crate::proxy;

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
            proxy::agent(
                &proxy::MqttConfig {
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
            proxy::local(
                &proxy::MqttConfig {
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
