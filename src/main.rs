use clap::Parser;
use const_str::concat;
use tracing::{debug, info, warn};
use shadow_rs::shadow;

shadow!(build);

mod log;
mod utils;

#[derive(Parser)]
#[command(version = concat!("v",build::PKG_VERSION,"-",build::SHORT_COMMIT))]
struct Args {
    /// Set config file path
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
