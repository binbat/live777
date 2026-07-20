//! livewrk — load testing tool for live777: WHIP publish and WHEP subscribe.
//! Named after `wrk`, the HTTP benchmarking tool.
//!
//! Usage:
//!   livewrk whip  --whip http://localhost:7777/whip/load --sessions 100 --duration 60
//!   livewrk whep  --whep http://localhost:7777/whep/load --sessions 100 --duration 60
//!   livewrk whep  --whep ... --verify-window 5   # also decode-verify one session at a time
//!
//! The `whip` subcommand and `whep --verify-window` require the `rsmpeg` feature.

use std::time::Duration;

use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing::{Level, info};

#[path = "../log.rs"]
mod log;
#[path = "../utils.rs"]
mod utils;

#[derive(Parser)]
#[command(name = "livewrk", version = version::version_with_features!())]
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
    /// http://localhost:7777/whep/load. Publish one first (e.g. `livewrk whip`
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

    /// Enable decode verification: a single rotating verifier decodes one
    /// session at a time for this many seconds, then switches to the next
    /// session, so decode cost stays constant regardless of the session
    /// count. Requires the rsmpeg feature.
    #[arg(long, value_name = "SECONDS")]
    verify_window: Option<u64>,

    /// Only report verification failures instead of failing the whole run
    #[arg(long)]
    verify_tolerant: bool,
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
        "livewrk={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

    match args.command {
        Command::Whip(whip_args) => run_whip(whip_args).await?,
        Command::Whep(whep_args) => run_whep(whep_args).await?,
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
    if stats.total_errors > 0 {
        println!("  Media write errors: {}", stats.total_errors);
    }
    if stats.total_nack_count > 0 || stats.total_pli_count > 0 {
        println!(
            "  RTCP feedback: {} NACK, {} PLI",
            stats.total_nack_count, stats.total_pli_count
        );
    }
    if stats.sessions_connected > 0 {
        let avg = stats.total_connected_duration / stats.sessions_connected as u32;
        println!("  Avg connected duration: {avg:.1?}");
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
        verify_window: args.verify_window.map(Duration::from_secs),
    };

    let config = livetwo::loadtest::LoadtestConfig {
        session_count: args.sessions,
        spawn_interval: Duration::from_millis(args.ramp_ms),
        duration: Some(Duration::from_secs(args.duration)),
    };

    let ct = CancellationToken::new();
    spawn_signal_handler(ct.clone());

    let (stats, verify) = livetwo::loadtest::whep::run(&config, params, ct).await?;
    print_stats("WHEP subscribe", &stats);
    if let Some(verify) = &verify {
        print_verify_stats(verify);
        if verify.windows_failed > 0 && !args.verify_tolerant {
            anyhow::bail!("{} verification window(s) failed", verify.windows_failed);
        }
    }
    Ok(())
}

fn print_verify_stats(verify: &livetwo::loadtest::whep::VerifyStats) {
    println!("  Decode verification:");
    if let Some(note) = &verify.note {
        println!("    {note}");
    }
    println!(
        "    Windows: {} total, {} ok, {} failed",
        verify.windows_total, verify.windows_ok, verify.windows_failed
    );
    println!(
        "    Sessions verified: {}, failed: {}",
        verify.sessions_covered.len(),
        verify.sessions_failed.len()
    );
    println!("    Frames decoded: {}", verify.frames_decoded);
    if let Some(error) = &verify.last_error {
        println!("    Last error: {error}");
    }
}
