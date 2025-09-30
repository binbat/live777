use clap::Parser;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

mod log;
mod utils;

use livecam::config::Config;

#[derive(Parser)]
#[command(name = "livecam", version)]
struct Args {
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let mut cfg: Config = livecam::utils::load("livecam", args.config);
    cfg.validate().unwrap();

    #[cfg(debug_assertions)]
    log::set(format!(
        "livecam={},net4mqtt={},tower_http=debug,webrtc=error",
        cfg.log.level, cfg.log.level
    ));

    #[cfg(not(debug_assertions))]
    log::set(format!(
        "livecam={},tower_http=info,webrtc=error",
        cfg.log.level
    ));

    warn!("set log level: {}", cfg.log.level);
    debug!("load config: {:?}", cfg);

    let listener = match tokio::net::TcpListener::bind(&cfg.http.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("bind to {} failed: {}", &cfg.http.listen, e);
            return;
        }
    };
    info!("server listening on : {}", &cfg.http.listen);

    let config_arc = Arc::new(RwLock::new(cfg));

    if let Err(e) = livecam::serve(config_arc, listener, utils::shutdown_signal()).await {
        tracing::error!("服务器运行时发生错误: {}", e);
    }

    info!("Server shutdown");
}
