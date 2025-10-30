use clap::Parser;
use tracing::{debug, info, warn};

mod log;
mod utils;

#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    liveion::metrics_register();
    let args = Args::parse();
    let cfg: liveion::config::Config = utils::load("live777".to_string(), args.config);
    cfg.validate().unwrap();
    log::set(format!(
        "live777={},liveion={},net4mqtt={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level, cfg.log.level
    ));
    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);
    let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    info!("Server listening on {}", addr);

    liveion::serve(cfg, listener, utils::shutdown_signal()).await;
    info!("Server shutdown");
}
