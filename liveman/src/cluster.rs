use std::net::ToSocketAddrs;
use tracing::{debug, info};

use crate::mem::Server;

pub async fn cluster_up(liveions: Vec<Server>) -> Vec<Server> {
    let mut results = Vec::new();

    for liveion in liveions.iter() {
        let mut cfg = liveion::config::Config::default();
        cfg.http.listen = liveion.url.to_socket_addrs().unwrap().next().unwrap();

        let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        cfg.webhook.node_addr = Some(addr);
        debug!("Liveion listening on {addr}");

        tokio::spawn(liveion::server_up(
            cfg,
            listener,
            shutdown_signal(addr.to_string()),
        ));

        results.push(Server {
            alias: if liveion.alias.is_empty() {
                format!("buildin-{}", addr.port())
            } else {
                liveion.alias.clone()
            },
            url: format!("http://{}", addr),
            pub_max: liveion.pub_max,
            sub_max: liveion.sub_max,
            ..Default::default()
        })
    }
    results
}

async fn shutdown_signal(addr: String) {
    let _ = signal::wait_for_stop_signal().await;
    info!("Build In Cluster Down: {}", addr);
}
