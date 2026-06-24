use clap::{Parser, Subcommand};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

mod log;
mod utils;

use livesrc::config::Config;

#[derive(Parser)]
#[command(name = "livesrc", version)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    Serve,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    match args.command {
        Some(Commands::Serve) | None => {}
    }

    let mut cfg: Config = livesrc::utils::load("livesrc", args.config);
    cfg.validate().unwrap();

    log::set(format!(
        "livesrc={},tower_http=info,webrtc=error",
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
    info!("livesrc server listening on: {}", &cfg.http.listen);

    let config_arc = Arc::new(RwLock::new(cfg));

    if let Err(e) = livesrc::serve(config_arc, listener, utils::shutdown_signal()).await {
        tracing::error!("server error: {}", e);
    }

    info!("livesrc server shutdown");
}
