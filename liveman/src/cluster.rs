use std::net::SocketAddr;
use std::str::FromStr;
use tracing::debug;

//#[cfg(debug_assertions)]
pub async fn cluster_up(num: u8) -> Vec<String> {
    let mut results = Vec::new();

    for _ in 0..=num {
        let mut cfg = liveion::config::Config::default();
        //cfg.http.listen = SocketAddr::from_str("0.0.0.0:0").unwrap();
        cfg.http.listen = SocketAddr::from_str("127.0.0.1:0").unwrap();

        let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        cfg.node_addr = Some(addr);
        println!("Listening on {addr}");

        tokio::spawn(liveion::server_up(cfg, listener, shutdown_signal()));

        results.push(addr.to_string());
    }
    results
}

async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}
