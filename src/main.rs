use clap::Parser;
use tracing::{debug, info, warn};

mod helper;

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
    let cfg = liveion::config::Config::parse(args.config);
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

    liveion::server_up(cfg, listener, helper::shutdown_signal()).await;
    info!("Server shutdown");
}
