use clap::Parser;
use tracing::{debug, info, warn};

mod log;
mod utils;

#[derive(Parser)]
#[command(version)]
struct Args {
    /// Set config file path
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut cfg: liveman::config::Config = utils::load("liveman".to_string(), args.config);
    cfg.validate().unwrap();

    #[cfg(debug_assertions)]
    log::set(format!(
        "liveman={},liveion={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level
    ));

    #[cfg(not(debug_assertions))]
    log::set(format!(
        "liveman={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level
    ));

    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);

    let listener = tokio::net::TcpListener::bind(cfg.http.listen)
        .await
        .unwrap();

    liveman::serve(cfg, listener, utils::shutdown_signal()).await;
    info!("Server shutdown");
}
