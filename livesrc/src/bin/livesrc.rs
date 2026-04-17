use clap::Parser;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

use livesrc::config::Config;

#[derive(Parser)]
#[command(name = "livesrc", version, about = "Lightweight camera source with WHEP streaming")]
struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let cfg: Config = livesrc::utils::load("livesrc", args.config);
    if let Err(e) = cfg.validate() {
        eprintln!("Config validation failed: {}", e);
        std::process::exit(1);
    }

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(format!(
            "livesrc={},tower_http=info,webrtc=error",
            cfg.log.level
        ))
        .init();

    warn!("set log level: {}", cfg.log.level);
    debug!("load config: {:?}", cfg);

    info!("livesrc starting...");
    if let Some(stream) = &cfg.stream {
        info!("  Stream ID: {}", stream.id);
        info!("  RTP Port: {}", stream.rtp_port);
    }
    if let Some(camera) = &cfg.camera {
        info!("  Camera: {}", camera.device);
    }
    info!("  Listen: {}", cfg.http.listen);

    let listener = match tokio::net::TcpListener::bind(&cfg.http.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind to {}: {}", cfg.http.listen, e);
            std::process::exit(1);
        }
    };

    let config = Arc::new(RwLock::new(cfg));

    if let Err(e) = livesrc::serve(config, listener, shutdown_signal()).await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }

    info!("livesrc shutdown");
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}
