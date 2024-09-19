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
    agent: Vec<u16>,
    local: Vec<u16>,

    ip: IpAddr,
    kcp: bool,
    echo: bool,
    broker: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: Vec::new(),
            local: Vec::new(),

            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            kcp: false,
            echo: false,
            broker: pick_unused_port().expect("No ports free"),
        }
    }
}

async fn helper_cluster_up(cfg: Config) {
    let mqtt_broker_host = cfg.ip;
    if cfg.echo {
        for port in cfg.agent.iter() {
            let addr = SocketAddr::new(cfg.ip, *port);
            thread::spawn(move || tokio_test::block_on(up_echo_tcp_server(addr)));
            thread::spawn(move || tokio_test::block_on(up_echo_udp_server(addr)));
        }
    }

    let broker_addr = SocketAddr::new(mqtt_broker_host, cfg.broker);
    thread::spawn(move || broker::up_mqtt_broker(broker_addr));
    wait_for_port_availabilty(broker_addr).await;

    for (id, port) in cfg.agent.into_iter().enumerate() {
        thread::spawn(move || {
            let addr = SocketAddr::new(cfg.ip, port);
            tokio_test::block_on(proxy::agent(
                &proxy::MqttConfig {
                    id: format!("test-proxy-agent-{}", id),
                    host: mqtt_broker_host.to_string(),
                    port: cfg.broker,
                },
                addr,
                MQTT_TOPIC_PREFIX,
                &id.to_string(),
            ))
        });
    }

    for (id, port) in cfg.local.into_iter().enumerate() {
        thread::spawn(move || {
            let addr = SocketAddr::new(cfg.ip, port);
            tokio_test::block_on(proxy::local(
                &proxy::MqttConfig {
                    id: format!("test-proxy-local-{}", id),
                    host: mqtt_broker_host.to_string(),
                    port: cfg.broker,
                },
                addr,
                MQTT_TOPIC_PREFIX,
                &id.to_string(),
                &id.to_string(),
                cfg.kcp,
            ))
        });
    }
}

#[tokio::test]
async fn test_udp_simple() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        agent: vec![agent_port],
        local: vec![local_port],

        ip,
        echo: true,
        broker: mqtt_broker_port,
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
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        agent: vec![agent_port],
        local: vec![local_port],

        ip,
        echo: true,
        broker: mqtt_broker_port,
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
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        agent: vec![agent_port],
        local: vec![local_port],

        ip,
        echo: true,
        broker: mqtt_broker_port,
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
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        agent: vec![agent_port],
        local: vec![local_port],

        ip,
        echo: true,
        broker: mqtt_broker_port,
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
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        agent: vec![agent_port],
        local: vec![local_port],

        ip,
        kcp: true,
        echo: true,
        broker: mqtt_broker_port,
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
async fn test_socks_simple() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let local_port: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        agent: vec![agent_port],

        ip,
        broker: mqtt_broker_port,
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

#[tokio::test]
async fn test_socks_multiple_server() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let agent_ports: Vec<u16> = (0..10)
        .map(|_| pick_unused_port().expect("No ports free"))
        .collect();
    let local_port: u16 = pick_unused_port().expect("No ports free");

    for (id, port) in agent_ports.iter().enumerate() {
        let agent_addr = SocketAddr::new(ip, *port);
        let message = id.to_string();
        thread::spawn(move || {
            tokio_test::block_on(up_http_server(agent_addr, &message));
        });
    }
    time::sleep(time::Duration::from_millis(100)).await;

    helper_cluster_up(Config {
        agent: agent_ports.clone(),

        ip,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;
    let tcp_over_kcp = false;
    let mqtt_broker_host = ip;

    let agent_id = "0";
    let local_id = "0";

    let local_addr = SocketAddr::new(ip, local_port);
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

    for (id, _port) in agent_ports.iter().enumerate() {
        let client = reqwest::Client::builder()
            .proxy(
                // References: https://github.com/seanmonstar/reqwest/issues/899
                reqwest::Proxy::all(format!("socks5h://{}", local_addr)).unwrap(),
            )
            .build()
            .unwrap();

        let res = client
            .get(format!("http://{}.test.local/", id))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        let body = res.text().await.unwrap();
        assert_eq!(body, id.to_string());
    }
}
