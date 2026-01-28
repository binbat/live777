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
#[command(about = "Multi-port UDP to DataChannel bridge for liveion")]
struct Args {
    /// Configuration file path
    #[arg(short, long, default_value = "bridge_multiport.toml")]
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
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
    
    info!("Starting liveion multi-port UDP bridge");
    println!("🚀 Starting liveion multi-port UDP bridge with message routing");
    
    // Load configuration
    let config = Config::load(&args.config).await?;
    info!("Loaded configuration from {:?}", args.config);
    println!("📋 Loaded configuration from {:?}", args.config);
    
    // Create and start the bridge
    let bridge = UdpDataChannelBridge::new(config).await?;
    println!("🌉 Multi-port bridge with message routing created successfully");
    
    // Handle shutdown gracefully
    tokio::select! {
        result = bridge.run() => {
            match result {
                Ok(_) => info!("Multi-port bridge stopped normally"),
                Err(e) => error!("Multi-port bridge error: {}", e),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, shutting down multi-port bridge...");
            println!("🛑 Shutting down multi-port bridge...");
        }
    }
    
    Ok(())
}