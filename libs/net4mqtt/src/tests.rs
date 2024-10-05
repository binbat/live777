use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::thread;

use anyhow::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time;

use crate::broker;

const MAX_BUFFER_SIZE: usize = 4096;

async fn check_port_availability(addr: SocketAddr) -> bool {
    TcpStream::connect(addr).await.is_ok()
}

async fn wait_for_port_availabilty(addr: SocketAddr) -> bool {
    let mut interval = time::interval(time::Duration::from_millis(1));
    loop {
        if check_port_availability(addr).await {
            return true;
        }
        interval.tick().await;
    }
}

async fn up_echo_udp_server(sock: UdpSocket) -> Result<(), Error> {
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        let (n, addr) = sock.recv_from(&mut buf).await?;
        let _ = sock.send_to(&buf[..n], addr).await?;
    }
}

async fn up_echo_tcp_server(listener: TcpListener) -> Result<(), Error> {
    loop {
        let (mut socket, _) = listener.accept().await?;
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

async fn up_add_udp_server(sock: UdpSocket) -> Result<(), Error> {
    let mut buf = [0; MAX_BUFFER_SIZE];
    loop {
        let (n, addr) = sock.recv_from(&mut buf).await?;
        let raw = String::from_utf8_lossy(&buf[..n]);
        let v: Vec<&str> = raw.split('+').collect();
        let num0 = v[0].parse::<u64>().unwrap_or(0);
        let num1 = v[1].parse::<u64>().unwrap_or(0);
        let r = num0 + num1;
        let _ = sock.send_to(r.to_string().as_bytes(), addr).await?;
    }
}

async fn up_add_tcp_server(listener: TcpListener) -> Result<(), Error> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            let mut buf = [0; MAX_BUFFER_SIZE];
            loop {
                match socket.read(&mut buf).await {
                    Ok(0) => return,
                    Ok(n) => {
                        let raw = String::from_utf8_lossy(&buf[..n]);
                        let v: Vec<&str> = raw.split('+').collect();
                        let num0 = v[0].parse::<u64>().unwrap_or(0);
                        let num1 = v[1].parse::<u64>().unwrap_or(0);
                        let r = num0 + num1;
                        if socket.write_all(r.to_string().as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                };
            }
        });
    }
}

async fn handle_request(body: &str, mut socket: tokio::net::TcpStream) -> Result<(), Error> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}

async fn up_http_server(listener: TcpListener, body: &str) -> Result<(), Error> {
    loop {
        let (socket, _) = listener.accept().await?;
        handle_request(body, socket).await?
    }
}

use portpicker::pick_unused_port;

use crate::proxy;

const MQTT_TOPIC_PREFIX: &str = "test";

struct Config {
    agent: u16,
    local_ports: u16,
    local_socks: u16,

    ip: IpAddr,
    kcp: bool,
    tcp: bool,
    broker: u16,
    target: Option<SocketAddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: 0,
            local_ports: 0,
            local_socks: 0,

            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            kcp: false,
            tcp: true,
            broker: pick_unused_port().expect("No ports free"),
            target: None,
        }
    }
}

async fn helper_cluster_up(cfg: Config) -> Vec<SocketAddr> {
    let mqtt_broker_host = cfg.ip;
    let addr = match cfg.target {
        Some(target) => target,
        None => {
            let addr = SocketAddr::new(cfg.ip, 0);
            if cfg.tcp {
                let listener = TcpListener::bind(addr).await.unwrap();
                let target = listener.local_addr().unwrap();
                thread::spawn(move || tokio_test::block_on(up_echo_tcp_server(listener)));
                target
            } else {
                let sock = UdpSocket::bind(addr).await.unwrap();
                let target = sock.local_addr().unwrap();
                thread::spawn(move || tokio_test::block_on(up_echo_udp_server(sock)));
                target
            }
        }
    };

    let broker_addr = SocketAddr::new(mqtt_broker_host, cfg.broker);
    thread::spawn(move || broker::up_mqtt_broker(broker_addr));
    wait_for_port_availabilty(broker_addr).await;

    for id in 0..cfg.agent {
        thread::spawn(move || {
            tokio_test::block_on(proxy::agent(
                &format!(
                    "mqtt://{}/{}?client_id=test-proxy-agent-{}",
                    SocketAddr::new(mqtt_broker_host, cfg.broker),
                    MQTT_TOPIC_PREFIX,
                    id
                ),
                addr,
                &id.to_string(),
                None,
                None,
            ))
        });
    }

    let mut addrs = Vec::new();

    for id in 0..cfg.local_ports {
        if cfg.tcp {
            let listener = TcpListener::bind(SocketAddr::new(cfg.ip, 0)).await.unwrap();
            addrs.push(listener.local_addr().unwrap());

            thread::spawn(move || {
                tokio_test::block_on(proxy::local_ports_tcp(
                    &format!(
                        "mqtt://{}/{}?client_id=test-proxy-local-{}",
                        SocketAddr::new(mqtt_broker_host, cfg.broker),
                        MQTT_TOPIC_PREFIX,
                        id
                    ),
                    listener,
                    &id.to_string(),
                    format!("local-{}", id).as_str(),
                    None,
                    None,
                    cfg.kcp,
                ))
            });
        } else {
            let sock = UdpSocket::bind(SocketAddr::new(cfg.ip, 0)).await.unwrap();
            addrs.push(sock.local_addr().unwrap());

            thread::spawn(move || {
                tokio_test::block_on(proxy::local_ports_udp(
                    &format!(
                        "mqtt://{}/{}?client_id=test-proxy-local-{}",
                        SocketAddr::new(mqtt_broker_host, cfg.broker),
                        MQTT_TOPIC_PREFIX,
                        id
                    ),
                    sock,
                    &id.to_string(),
                    format!("local-{}", id).as_str(),
                    None,
                    None,
                ))
            });
        }
    }

    for id in 0..cfg.local_socks {
        let listener = TcpListener::bind(SocketAddr::new(cfg.ip, 0)).await.unwrap();
        addrs.push(listener.local_addr().unwrap());

        thread::spawn(move || {
            tokio_test::block_on(proxy::local_socks(
                &format!(
                    "mqtt://{}/{}?client_id=test-proxy-socks-{}",
                    SocketAddr::new(mqtt_broker_host, cfg.broker),
                    MQTT_TOPIC_PREFIX,
                    id
                ),
                listener,
                &id.to_string(),
                format!("socks-{}", id).as_str(),
                None,
                None,
                cfg.kcp,
            ))
        });
    }

    addrs
}

#[tokio::test]
async fn test_udp_simple() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        tcp: false,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
    sock.connect(addrs[0]).await.unwrap();
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
async fn test_udp_add() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let target = sock.local_addr().unwrap();
    thread::spawn(move || tokio_test::block_on(up_add_udp_server(sock)));

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        tcp: false,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
    sock.connect(addrs[0]).await.unwrap();
    let mut buf = [0; MAX_BUFFER_SIZE];
    let test_msg = b"1+2";
    sock.send(test_msg).await.unwrap();
    let len = sock.recv(&mut buf).await.unwrap();
    assert_eq!(std::str::from_utf8(&buf[..len]), Ok("3"));

    let test_msg2 = b"123456+543210";
    sock.send(test_msg2).await.unwrap();
    let len = sock.recv(&mut buf).await.unwrap();
    assert_eq!(std::str::from_utf8(&buf[..len]), Ok("666666"));
}

#[tokio::test]
async fn test_udp_ipv6() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        tcp: false,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
    sock.connect(addrs[0]).await.unwrap();

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
    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        tcp: false,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let sock2 = UdpSocket::bind(SocketAddr::new(ip, 0)).await.unwrap();
    sock.connect(addrs[0]).await.unwrap();
    sock2.connect(addrs[0]).await.unwrap();

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
async fn test_tcp_echo() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;

    let mut socket = TcpStream::connect(addrs[0]).await.unwrap();

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
async fn test_tcp_add() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let target = listener.local_addr().unwrap();
    thread::spawn(move || tokio_test::block_on(up_add_tcp_server(listener)));

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let mut socket = TcpStream::connect(addrs[0]).await.unwrap();

    let mut buf = [0; MAX_BUFFER_SIZE];

    let test_msg = b"1+2";
    socket.write_all(test_msg).await.unwrap();
    let len = socket.read(&mut buf).await.unwrap();
    assert_eq!(std::str::from_utf8(&buf[..len]), Ok("3"));

    let test_msg2 = b"123456+543210";
    socket.write_all(test_msg2).await.unwrap();
    let len = socket.read(&mut buf).await.unwrap();
    assert_eq!(std::str::from_utf8(&buf[..len]), Ok("666666"));
}

#[tokio::test]
async fn test_kcp_add() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let target = listener.local_addr().unwrap();
    thread::spawn(move || tokio_test::block_on(up_add_tcp_server(listener)));

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        kcp: true,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let mut socket = TcpStream::connect(addrs[0]).await.unwrap();

    let mut buf = [0; MAX_BUFFER_SIZE];

    let test_msg = b"1+2";
    socket.write_all(test_msg).await.unwrap();
    let len = socket.read(&mut buf).await.unwrap();
    assert_eq!(std::str::from_utf8(&buf[..len]), Ok("3"));

    let test_msg2 = b"123456+543210";
    socket.write_all(test_msg2).await.unwrap();
    let len = socket.read(&mut buf).await.unwrap();
    assert_eq!(std::str::from_utf8(&buf[..len]), Ok("666666"));
}

#[tokio::test]
async fn test_tcp_echo_restart() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let target = SocketAddr::new(ip, agent_port);

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;

    for i in 0..10 {
        time::sleep(time::Duration::from_millis(10)).await;
        let listener = TcpListener::bind(target).await.unwrap();
        let handle = tokio::spawn(up_echo_tcp_server(listener));

        let mut socket = TcpStream::connect(addrs[0]).await.unwrap();

        let mut buf = [0; MAX_BUFFER_SIZE];
        let test_msg = format!("hello, world: {}", i);
        socket.write_all(test_msg.as_bytes()).await.unwrap();
        let len = socket.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg.as_bytes());

        let test_msg2 = format!("the end: {}", i);
        socket.write_all(test_msg2.as_bytes()).await.unwrap();
        let len = socket.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg2.as_bytes());

        handle.abort();
    }
}

#[tokio::test]
async fn test_kcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        kcp: true,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;

    let mut socket = TcpStream::connect(addrs[0]).await.unwrap();

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
async fn test_kcp_echo_restart() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let target = SocketAddr::new(ip, agent_port);

    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_ports: 1,

        ip,
        kcp: true,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;

    for i in 0..10 {
        time::sleep(time::Duration::from_millis(10)).await;
        let listener = TcpListener::bind(target).await.unwrap();
        let handle = tokio::spawn(up_echo_tcp_server(listener));

        let mut socket = TcpStream::connect(addrs[0]).await.unwrap();

        let mut buf = [0; MAX_BUFFER_SIZE];
        let test_msg = format!("hello, world: {}", i);
        socket.write_all(test_msg.as_bytes()).await.unwrap();
        let len = socket.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg.as_bytes());

        let test_msg2 = format!("the end: {}", i);
        socket.write_all(test_msg2.as_bytes()).await.unwrap();
        let len = socket.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], test_msg2.as_bytes());

        handle.abort();
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn test_socks_simple() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let target = listener.local_addr().unwrap();
    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_socks: 1,

        ip,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;
    time::sleep(time::Duration::from_millis(10)).await;

    let message = "Hello World!";
    thread::spawn(move || tokio_test::block_on(up_http_server(listener, message)));

    let local_addr = addrs[0];
    let client = reqwest::Client::builder()
        .connect_timeout(time::Duration::from_secs(1))
        .timeout(time::Duration::from_secs(1))
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
        .connect_timeout(time::Duration::from_secs(1))
        .timeout(time::Duration::from_secs(1))
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

#[cfg(not(windows))]
#[tokio::test]
async fn test_socks_restart() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let target = SocketAddr::new(ip, agent_port);

    //let target = listener.local_addr().unwrap();
    let addrs = helper_cluster_up(Config {
        agent: 1,
        local_socks: 1,

        ip,
        broker: mqtt_broker_port,
        target: Some(target),
        ..Default::default()
    })
    .await;

    let local_addr = addrs[0];
    for _ in 0..10 {
        time::sleep(time::Duration::from_millis(10)).await;
        let message = "Hello World!";
        let listener = TcpListener::bind(target).await.unwrap();
        let handle = tokio::spawn(up_http_server(listener, message));

        let client = reqwest::Client::builder()
            .connect_timeout(time::Duration::from_secs(1))
            .timeout(time::Duration::from_secs(1))
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
            .connect_timeout(time::Duration::from_secs(1))
            .timeout(time::Duration::from_secs(1))
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

        handle.abort();
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn test_socks_multiple_server() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = 1883;
    let n = 10;

    let addrs = helper_cluster_up(Config {
        local_socks: 1,

        ip,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;

    for id in 0..n {
        let message = id.to_string();

        let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        thread::spawn(move || tokio_test::block_on(up_http_server(listener, &message)));
        thread::spawn(move || {
            tokio_test::block_on(proxy::agent(
                &format!(
                    "mqtt://{}/{}?client_id=test-proxy-agent-{}",
                    SocketAddr::new(ip, mqtt_broker_port),
                    MQTT_TOPIC_PREFIX,
                    id
                ),
                addr,
                &id.to_string(),
                None,
                None,
            ))
        });
    }
    time::sleep(time::Duration::from_millis(100)).await;

    let local_addr = addrs[0];

    for id in 0..n {
        let client = reqwest::Client::builder()
            .connect_timeout(time::Duration::from_secs(1))
            .timeout(time::Duration::from_secs(1))
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

#[tokio::test]
async fn test_xdata() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mqtt_broker_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port: u16 = pick_unused_port().expect("No ports free");
    let agent_port_1: u16 = pick_unused_port().expect("No ports free");
    let agent_port_2: u16 = pick_unused_port().expect("No ports free");
    helper_cluster_up(Config {
        ip,
        kcp: true,
        broker: mqtt_broker_port,
        ..Default::default()
    })
    .await;

    let msg_1: Vec<u8> = "xxx".bytes().collect();
    let msg_2: Vec<u8> = "yyyyyyyy".bytes().collect();
    let msg_3: Vec<u8> = "333".bytes().collect();
    let msg_4: Vec<u8> = "4444".bytes().collect();

    let msg_1_clone = msg_1.clone();
    let msg_2_clone = msg_2.clone();
    let msg_3_clone = msg_3.clone();
    let msg_4_clone = msg_4.clone();

    let (sender, mut receiver) = tokio::sync::mpsc::channel::<(String, String, Vec<u8>)>(10);

    thread::spawn(move || {
        let id = 0;
        let addr = SocketAddr::new(ip, agent_port);
        tokio_test::block_on(proxy::agent(
            &format!(
                "mqtt://{}/{}?client_id=test-proxy-agent-{}",
                SocketAddr::new(ip, mqtt_broker_port),
                MQTT_TOPIC_PREFIX,
                id
            ),
            addr,
            &id.to_string(),
            None,
            Some(sender),
        ))
    });

    time::sleep(time::Duration::from_millis(100)).await;

    thread::spawn(move || {
        let id = 1;
        let addr = SocketAddr::new(ip, agent_port_1);
        tokio_test::block_on(proxy::agent(
            &format!(
                "mqtt://{}/{}?client_id=test-proxy-agent-{}",
                SocketAddr::new(ip, mqtt_broker_port),
                MQTT_TOPIC_PREFIX,
                id
            ),
            addr,
            &id.to_string(),
            Some((msg_1_clone, None)),
            None,
        ))
    });
    time::sleep(time::Duration::from_millis(100)).await;

    thread::spawn(move || {
        let id = 2;
        let addr = SocketAddr::new(ip, agent_port_2);
        tokio_test::block_on(proxy::agent(
            &format!(
                "mqtt://{}/{}?client_id=test-proxy-agent-{}",
                SocketAddr::new(ip, mqtt_broker_port),
                MQTT_TOPIC_PREFIX,
                id
            ),
            addr,
            &id.to_string(),
            Some((msg_2_clone, None)),
            None,
        ))
    });

    time::sleep(time::Duration::from_millis(100)).await;

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    thread::spawn(move || {
        let id = "local-x";
        tokio_test::block_on(proxy::local_ports_tcp(
            &format!(
                "mqtt://{}/{}?client_id=test-proxy-local-{}",
                SocketAddr::new(ip, mqtt_broker_port),
                MQTT_TOPIC_PREFIX,
                id
            ),
            listener,
            id,
            id,
            Some((msg_3_clone, None)),
            None,
            false,
        ))
    });

    time::sleep(time::Duration::from_millis(100)).await;

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    thread::spawn(move || {
        let id = "socks-x";
        tokio_test::block_on(proxy::local_ports_tcp(
            &format!(
                "mqtt://{}/{}?client_id=test-proxy-local-{}",
                SocketAddr::new(ip, mqtt_broker_port),
                MQTT_TOPIC_PREFIX,
                id
            ),
            listener,
            id,
            id,
            Some((msg_4_clone, None)),
            None,
            false,
        ))
    });

    let (agent_id, _local_id, r1) = receiver.recv().await.unwrap();
    assert_eq!(msg_1, r1);
    assert_eq!("1", agent_id);
    let (agent_id, _local_id, r2) = receiver.recv().await.unwrap();
    assert_eq!("2", agent_id);
    assert_eq!(msg_2, r2);

    let (agent_id, local_id, data) = receiver.recv().await.unwrap();
    assert_eq!(msg_3, data);
    assert_eq!("-", agent_id);
    assert_eq!("local-x", local_id);
    let (agent_id, local_id, data) = receiver.recv().await.unwrap();
    assert_eq!("-", agent_id);
    assert_eq!(msg_4, data);
    assert_eq!("socks-x", local_id);
}
