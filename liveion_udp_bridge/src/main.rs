use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing::{info, error};

mod config;
mod udp_server;
mod datachannel_client;
mod bridge;

use config::Config;
use bridge::UdpDataChannelBridge;

#[derive(Parser)]
#[command(name = "liveion-udp-bridge")]
#[command(about = "UDP to DataChannel bridge for liveion")]
struct Args {
    /// Configuration file path
    #[arg(short, long, default_value = "bridge.toml")]
    config: PathBuf,
    
    /// Verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(if args.verbose { 
            tracing::Level::DEBUG 
        } else { 
            tracing::Level::INFO 
        })
        .with_target(false)  // Don't show module paths
        .with_thread_ids(false)  // Don't show thread IDs
        .with_file(false)  // Don't show file names
        .with_line_number(false)  // Don't show line numbers
        .init();
    
    info!("Starting liveion UDP bridge");
    println!("🚀 Starting liveion UDP bridge");
    
    // Load configuration
    let config = Config::load(&args.config).await?;
    info!("Loaded configuration from {:?}", args.config);
    println!("📋 Loaded configuration from {:?}", args.config);
    
    // Create and start the bridge
    let bridge = UdpDataChannelBridge::new(config).await?;
    println!("🌉 Bridge created successfully");
    
    // Handle shutdown gracefully
    tokio::select! {
        result = bridge.run() => {
            match result {
                Ok(_) => info!("Bridge stopped normally"),
                Err(e) => error!("Bridge error: {}", e),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down...");
        }
    }
    
    Ok(())
}