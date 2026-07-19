//! Load test: DataChannel <-> UDP forwarding throughput, latency and bidirectional traffic.
//!
//! Usage:
//!   cargo run --release --features source --bin datachannel_loadtest -- all
//!   cargo run --release --features source --bin datachannel_loadtest -- throughput
//!   cargo run --release --features source --bin datachannel_loadtest -- latency
//!   cargo run --release --features source --bin datachannel_loadtest -- bidirectional
//!
//! Custom params (env vars):
//!   LOADTEST_PACKET_SIZE=1400  LOADTEST_PACKET_COUNT=10000  cargo run ...
//!   LOADTEST_LATENCY_ROUNDS=200                              cargo run ...

mod loadtest_channel;

use loadtest_channel::{ChannelLoadtestParams, ChannelMode};

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}

fn params_from_env() -> Result<ChannelLoadtestParams, String> {
    let mode: ChannelMode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "all".to_string())
        .parse()?;

    Ok(ChannelLoadtestParams {
        mode,
        packet_size: env_usize("LOADTEST_PACKET_SIZE").unwrap_or(1400),
        packet_count: env_usize("LOADTEST_PACKET_COUNT").unwrap_or(10000),
        warmup_packets: env_usize("LOADTEST_WARMUP_PKTS").unwrap_or(3),
        latency_rounds: env_usize("LOADTEST_LATENCY_ROUNDS").unwrap_or(200),
        window: env_usize("LOADTEST_WINDOW"),
        bind_host: std::env::var("LOADTEST_BIND_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        target_host: std::env::var("LOADTEST_TARGET_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let params = match params_from_env() {
        Ok(params) => params,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("usage: datachannel_loadtest [all|throughput|latency|bidirectional]");
            std::process::exit(2);
        }
    };

    loadtest_channel::print_environment_hint(&params);
    loadtest_channel::run(&params).await
}
