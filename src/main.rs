use clap::Parser;
use local_ip_address::local_ip;
use std::net::SocketAddr;
use std::str::FromStr;
use tracing::{debug, info, warn};

#[derive(Parser)]
#[command(version)]
struct Args {
    /// Set config file path
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    liveion::metrics_register();
    let args = Args::parse();
    let mut cfg = liveion::config::Config::parse(args.config);
    utils::set_log(format!(
        "live777={},liveion={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level
    ));
    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);
    let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    info!("Server listening on {}", addr);
    if cfg.node_addr.is_none() {
        let port = addr.port();
        cfg.node_addr =
            Some(SocketAddr::from_str(&format!("{}:{}", local_ip().unwrap(), port)).unwrap());
        warn!(
            "config node_addr not set, auto detect local_ip_port : {:?}",
            cfg.node_addr.unwrap()
        );
    }

    liveion::server_up(cfg, listener, shutdown_signal()).await;
    info!("Server shutdown");
}

async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}
