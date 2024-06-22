use std::net::SocketAddr;
use tracing::debug;

//#[cfg(debug_assertions)]
pub async fn cluster_up(count: u16, address: SocketAddr) -> Vec<String> {
    let mut results = Vec::new();

    for _ in 1..=count {
        let mut cfg = liveion::config::Config::default();
        cfg.http.listen = address;

        let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        cfg.node_addr = Some(addr);
        debug!("Liveion listening on {addr}");

        tokio::spawn(liveion::server_up(cfg, listener, shutdown_signal()));
        results.push(addr.to_string());
    }
    results
}

async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}
