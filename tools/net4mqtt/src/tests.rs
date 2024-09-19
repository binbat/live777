use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::thread;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
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

async fn handle_request(body: &str, mut socket: tokio::net::TcpStream) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    socket.write_all(response.as_bytes()).await.unwrap();
}

async fn up_http_server(listen: SocketAddr, body: &str) {
    let listener = TcpListener::bind(listen).await.unwrap();
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        handle_request(body, socket).await;
    }
}

use portpicker::pick_unused_port;

use crate::proxy;

const MQTT_TOPIC_PREFIX: &str = "test";

struct Config {
    agent: bool,
    local: bool,

    ip: IpAddr,
    kcp: bool,
    echo: bool,
    broker: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: true,
            local: true,

            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            kcp: false,
            echo: false,
            broker: false,
        }
    }
}

async fn helper_cluster_up(cfg: Config) -> (u16, u16, u16) {
    let mqtt_broker_host = cfg.ip;
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");

    let agent_id = "0";
    let local_id = "0";

    let agent_addr = SocketAddr::new(cfg.ip, agent_port);
    let local_addr = SocketAddr::new(cfg.ip, local_port);
    let broker_addr = SocketAddr::new(mqtt_broker_host, mqtt_broker_port);

    if cfg.echo {
        thread::spawn(move || tokio_test::block_on(up_echo_tcp_server(agent_addr)));
        thread::spawn(move || tokio_test::block_on(up_echo_udp_server(agent_addr)));
    }

    if cfg.broker {
        thread::spawn(move || broker::up_mqtt_broker(broker_addr));
        wait_for_port_availabilty(broker_addr).await;
    }

    if cfg.agent {
        thread::spawn(move || {
            tokio_test::block_on(proxy::agent(
                &proxy::MqttConfig {
                    id: format!("test-proxy-agent-{}", agent_id),
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                agent_addr,
                MQTT_TOPIC_PREFIX,
                agent_id,
            ))
        });
    }

    if cfg.local {
        thread::spawn(move || {
            tokio_test::block_on(proxy::local(
                &proxy::MqttConfig {
                    id: format!("test-proxy-local-{}", local_id),
                    host: mqtt_broker_host.to_string(),
                    port: mqtt_broker_port,
                },
                local_addr,
                MQTT_TOPIC_PREFIX,
                agent_id,
                local_id,
                cfg.kcp,
            ))
        });
    }
    (agent_port, local_port, mqtt_broker_port)
}

#[tokio::test]
async fn test_udp_simple() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let (_agent_port, local_port, _broker_port) = helper_cluster_up(Config {
        ip,
        echo: true,
        broker: true,
        ..Default::default()
    })
    .await;
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
    let (_agent_port, local_port, _broker_port) = helper_cluster_up(Config {
        ip,
        echo: true,
        broker: true,
        ..Default::default()
    })
    .await;
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
    let (_agent_port, local_port, _broker_port) = helper_cluster_up(Config {
        ip,
        echo: true,
        broker: true,
        ..Default::default()
    })
    .await;
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
    let (_agent_port, local_port, _broker_port) = helper_cluster_up(Config {
        ip,
        echo: true,
        broker: true,
        ..Default::default()
    })
    .await;
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

#[tokio::test]
async fn test_kcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let (_agent_port, local_port, _broker_port) = helper_cluster_up(Config {
        ip,
        kcp: true,
        echo: true,
        broker: true,
        ..Default::default()
    })
    .await;
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

#[tokio::test]
async fn test_socks() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let (agent_port, local_port, mqtt_broker_port) = helper_cluster_up(Config {
        local: false,

        ip,
        broker: true,
        ..Default::default()
    })
    .await;
    let tcp_over_kcp = false;
    let mqtt_broker_host = ip;

    let agent_id = "0";
    let local_id = "0";

    let agent_addr = SocketAddr::new(ip, agent_port);
    let local_addr = SocketAddr::new(ip, local_port);

    let message = "Hello World!";
    thread::spawn(move || {
        tokio_test::block_on(up_http_server(agent_addr, message));
    });

    thread::spawn(move || {
        tokio_test::block_on(proxy::local_socks(
            &proxy::MqttConfig {
                id: format!("test-proxy-local-{}", local_id),
                host: mqtt_broker_host.to_string(),
                port: mqtt_broker_port,
            },
            local_addr,
            MQTT_TOPIC_PREFIX,
            agent_id,
            local_id,
            tcp_over_kcp,
        ))
    });

    time::sleep(time::Duration::from_millis(10)).await;

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(format!("socks5://{}", local_addr)).unwrap())
        .build()
        .unwrap();

    let res = client
        .get(format!("http://{}/", local_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    let body = res.text().await.unwrap();
    assert_eq!(body, message);
}
