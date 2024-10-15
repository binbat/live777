use std::net::ToSocketAddrs;
use tracing::{debug, info};

use crate::config::Node;

pub async fn cluster_up(liveions: Vec<Node>) -> Vec<Node> {
    let mut results = Vec::new();

    for liveion in liveions.iter() {
        let mut cfg = liveion::config::Config::default();
        cfg.http.listen = liveion.url.to_socket_addrs().unwrap().next().unwrap();

        let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        debug!("Liveion listening on {addr}");

        tokio::spawn(liveion::serve(
            cfg,
            listener,
            shutdown_signal(addr.to_string()),
        ));

        results.push(Node {
            alias: if liveion.alias.is_empty() {
                format!("buildin-{}", addr.port())
            } else {
                liveion.alias.clone()
            },
            url: format!("http://{}", addr),
            ..Default::default()
        })
    }
    results
}

async fn shutdown_signal(addr: String) {
    let _ = signal::wait_for_stop_signal().await;
    info!("Build In Cluster Down: {}", addr);
}
