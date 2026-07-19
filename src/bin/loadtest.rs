//! Load testing tool for live777: WHIP publish, WHEP subscribe, and
//! DataChannel <-> UDP forwarding benchmarks.
//!
//! Usage:
//!   loadtest whip  --whip http://localhost:7777/whip/load --sessions 100 --duration 60
//!   loadtest whep  --whep http://localhost:7777/whep/load --sessions 100 --duration 60
//!   loadtest channel [all|throughput|latency|bidirectional]
//!
//! The `whip` subcommand requires the `rsmpeg` feature; the `channel`
//! subcommand requires the `source` feature and runs a self-contained
//! topology (in-process liveion, no external server needed).

use std::time::Duration;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing::{Level, info};

#[cfg(feature = "source")]
mod loadtest_channel;
#[path = "../log.rs"]
mod log;
#[path = "../utils.rs"]
mod utils;

#[derive(Parser)]
#[command(name = "loadtest", version = version::version_with_features!())]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Publish N concurrent synthetic WHIP streams (requires the rsmpeg feature)
    Whip(WhipArgs),
    /// Subscribe N concurrent WHEP sessions to one already-published stream
    Whep(WhepArgs),
    /// DataChannel <-> UDP forwarding benchmark, self-contained (requires the source feature)
    Channel(ChannelArgs),
}

#[derive(clap::Args)]
struct WhipArgs {
    /// The WHIP server endpoint base URL; each session appends `-N` to the
    /// last path segment. e.g.: http://localhost:7777/whip/load
    #[arg(short, long)]
    whip: String,

    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,

    /// Number of concurrent WHIP publish sessions
    #[arg(long, default_value_t = 100)]
    sessions: usize,

    /// Milliseconds to wait between spawning each session (ramp-up)
    #[arg(long, default_value_t = 10)]
    ramp_ms: u64,

    /// Overall run duration in seconds; sessions are stopped afterwards
    #[arg(long, default_value_t = 60)]
    duration: u64,

    /// Video codec: vp8, vp9, h264, h265, av1
    #[arg(long = "vcodec", default_value = "vp8")]
    video_codec: String,

    /// Audio codec: opus, g722 (omit for no audio)
    #[arg(long = "acodec")]
    audio_codec: Option<String>,

    /// Video width in pixels
    #[arg(long, default_value_t = 640)]
    width: u32,

    /// Video height in pixels
    #[arg(long, default_value_t = 480)]
    height: u32,

    /// Video frame rate
    #[arg(long, default_value_t = 30)]
    fps: u32,

    /// STUN server URL used for ICE gathering
    #[arg(long, default_value = "stun:stun.l.google.com:19302")]
    stun_server: String,
}

#[derive(clap::Args)]
struct WhepArgs {
    /// The WHEP endpoint of an already-published stream, e.g.
    /// http://localhost:7777/whep/load. Publish one first (e.g. `loadtest whip`
    /// or `whipsynth`).
    #[arg(short, long)]
    whep: String,

    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,

    /// Number of concurrent WHEP subscribe sessions
    #[arg(long, default_value_t = 100)]
    sessions: usize,

    /// Milliseconds to wait between spawning each session (ramp-up)
    #[arg(long, default_value_t = 10)]
    ramp_ms: u64,

    /// Overall run duration in seconds; sessions are stopped afterwards
    #[arg(long, default_value_t = 60)]
    duration: u64,
}

#[derive(clap::Args)]
struct ChannelArgs {
    /// Which measurement to run
    #[arg(default_value = "all")]
    mode: String,

    /// UDP payload size in bytes
    #[arg(long, default_value_t = 1400)]
    packet_size: usize,

    /// Number of packets per throughput run
    #[arg(long, default_value_t = 10000)]
    packet_count: usize,

    /// Warmup packets before measuring (also a readiness probe)
    #[arg(long, default_value_t = 3)]
    warmup_packets: usize,

    /// Latency ping-pong rounds
    #[arg(long, default_value_t = 200)]
    latency_rounds: usize,

    /// Max in-flight packets (sliding window); default derives from packet size
    #[arg(long)]
    window: Option<usize>,

    /// Local address to bind the UDP receivers to
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    /// Address to send the UDP test traffic to
    #[arg(long, default_value = "127.0.0.1")]
    target: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(e) = run().await {
        eprintln!("Error: {e:?}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run() -> Result<()> {
    let args = Args::parse();

    let (log_level, webrtc_level) = match args.verbose {
        0 => (Level::WARN, Level::ERROR),
        1 => (Level::INFO, Level::WARN),
        2 => (Level::DEBUG, Level::INFO),
        _ => (Level::TRACE, Level::DEBUG),
    };

    log::set(format!(
        "loadtest={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

    match args.command {
        Command::Whip(whip_args) => run_whip(whip_args).await?,
        Command::Whep(whep_args) => run_whep(whep_args).await?,
        Command::Channel(channel_args) => run_channel(channel_args).await?,
    }

    info!("=== Graceful shutdown completed ===");
    Ok(())
}

/// Cancel `ct` on SIGINT/SIGTERM so a running loadtest stops and reports.
fn spawn_signal_handler(ct: CancellationToken) {
    tokio::spawn(async move {
        utils::shutdown_signal().await;
        info!("Shutdown signal received, stopping loadtest");
        ct.cancel();
    });
}

fn print_stats(kind: &str, stats: &livetwo::loadtest::LoadtestStats) {
    println!("\n══════════════════════════════════════════════");
    println!("  {kind} loadtest results");
    println!(
        "  Sessions: {} total, {} connected, {} failed",
        stats.sessions_total, stats.sessions_connected, stats.sessions_failed
    );
    println!(
        "  Packets: {}, bytes: {} ({:.2} MB)",
        stats.total_packets,
        stats.total_bytes,
        stats.total_bytes as f64 / 1_000_000.0
    );
    if stats.total_nack_count > 0 || stats.total_pli_count > 0 {
        println!(
            "  RTCP feedback: {} NACK, {} PLI",
            stats.total_nack_count, stats.total_pli_count
        );
    }
    println!("══════════════════════════════════════════════\n");
}

#[cfg(feature = "rsmpeg")]
async fn run_whip(args: WhipArgs) -> Result<()> {
    use anyhow::{Context, anyhow};

    let video_codec_cli = cli::codec_from_str(&args.video_codec)
        .with_context(|| format!("Invalid video codec: {}", args.video_codec))?;
    let video_codec = livetwo::source::VideoCodec::from_cli(video_codec_cli)
        .ok_or_else(|| anyhow!("Unsupported video codec: {}", args.video_codec))?;

    let audio_codec = match &args.audio_codec {
        Some(name) => {
            let audio_codec_cli = cli::codec_from_str(name)
                .with_context(|| format!("Invalid audio codec: {}", name))?;
            Some(
                livetwo::source::AudioCodec::from_cli(audio_codec_cli)
                    .ok_or_else(|| anyhow!("Unsupported audio codec: {}", name))?,
            )
        }
        None => None,
    };

    let publisher_config = livetwo::whipsynth::PublisherConfig {
        whip_url: args.whip,
        token: args.token,
        video_codec,
        audio_codec,
        width: args.width,
        height: args.height,
        fps: args.fps,
        duration: None,
        stun_server: args.stun_server,
    };

    let config = livetwo::loadtest::LoadtestConfig {
        session_count: args.sessions,
        spawn_interval: Duration::from_millis(args.ramp_ms),
        duration: Some(Duration::from_secs(args.duration)),
    };

    let ct = CancellationToken::new();
    spawn_signal_handler(ct.clone());

    let stats = livetwo::loadtest::whip::run(&config, publisher_config, ct).await?;
    print_stats("WHIP publish", &stats);
    Ok(())
}

#[cfg(not(feature = "rsmpeg"))]
async fn run_whip(_args: WhipArgs) -> Result<()> {
    anyhow::bail!(
        "the `whip` subcommand requires the `rsmpeg` feature; rebuild with --features rsmpeg"
    )
}

async fn run_whep(args: WhepArgs) -> Result<()> {
    let params = livetwo::loadtest::whep::WhepLoadParams {
        whep_url: args.whep,
        token: args.token,
    };

    let config = livetwo::loadtest::LoadtestConfig {
        session_count: args.sessions,
        spawn_interval: Duration::from_millis(args.ramp_ms),
        duration: Some(Duration::from_secs(args.duration)),
    };

    let ct = CancellationToken::new();
    spawn_signal_handler(ct.clone());

    let stats = livetwo::loadtest::whep::run(&config, params, ct).await?;
    print_stats("WHEP subscribe", &stats);
    Ok(())
}

#[cfg(feature = "source")]
async fn run_channel(args: ChannelArgs) -> Result<()> {
    let params = loadtest_channel::ChannelLoadtestParams {
        mode: args.mode.parse().map_err(|e: String| anyhow::anyhow!(e))?,
        packet_size: args.packet_size,
        packet_count: args.packet_count,
        warmup_packets: args.warmup_packets,
        latency_rounds: args.latency_rounds,
        window: args.window,
        bind_host: args.bind,
        target_host: args.target,
    };

    loadtest_channel::print_environment_hint(&params);
    loadtest_channel::run(&params)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

#[cfg(not(feature = "source"))]
async fn run_channel(_args: ChannelArgs) -> Result<()> {
    anyhow::bail!(
        "the `channel` subcommand requires the `source` feature; rebuild with --features source"
    )
}
