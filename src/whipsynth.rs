use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser};
use tokio_util::sync::CancellationToken;
use tracing::{Level, info};

mod log;
mod utils;

#[derive(Parser)]
#[command(name = "whipsynth", version = version::version_with_features!())]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,

    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
    #[arg(short, long)]
    whip: String,

    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,

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

    /// Run for the specified number of seconds, then exit (default: run until interrupted)
    #[arg(long)]
    duration: Option<u64>,

    /// Number of concurrent WHIP sessions to publish (loadtest mode).
    #[arg(long, default_value_t = 1, hide = true)]
    count: usize,

    /// Milliseconds to wait between spawning each loadtest session.
    #[arg(long, default_value_t = 100, hide = true)]
    spawn_interval_ms: u64,

    /// Overall timeout in seconds. If the publisher cannot connect or the
    /// loadtest cannot finish within this time, exit with an error.
    #[arg(long, default_value_t = 60)]
    timeout: u64,

    /// STUN server URL used for ICE gathering.
    #[arg(long, default_value = "stun:stun.l.google.com:19302")]
    stun_server: String,
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
        "whipsynth={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

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

    let config = livetwo::whipsynth::PublisherConfig {
        whip_url: args.whip.clone(),
        token: args.token.clone(),
        video_codec,
        audio_codec,
        width: args.width,
        height: args.height,
        fps: args.fps,
        duration: args.duration.map(Duration::from_secs),
        stun_server: args.stun_server.clone(),
    };

    info!(
        whip_url = %args.whip,
        video_codec = %args.video_codec,
        audio_codec = ?args.audio_codec,
        width = args.width,
        height = args.height,
        fps = args.fps,
        duration = ?args.duration,
        count = args.count,
        spawn_interval_ms = args.spawn_interval_ms,
        "Starting WHIP synthetic stream generator"
    );

    let ct = CancellationToken::new();
    let timeout = Duration::from_secs(args.timeout);

    if args.count > 1 {
        let loadtest_config = livetwo::whipsynth::LoadtestConfig {
            publisher_config: config,
            session_count: args.count,
            spawn_interval: Duration::from_millis(args.spawn_interval_ms),
        };

        tokio::select! {
            result = livetwo::whipsynth::run_loadtest(loadtest_config, ct.clone()) => {
                let stats = result?;
                info!(?stats, "Loadtest finished");
            }
            _ = tokio::time::sleep(timeout) => {
                ct.cancel();
                return Err(anyhow!("loadtest timed out after {:?}", timeout));
            }
            _ = utils::shutdown_signal() => {
                info!("Shutdown signal received, stopping loadtest");
                ct.cancel();
            }
        }
    } else {
        let publisher = livetwo::whipsynth::Publisher::new(config);

        if let Some(duration) = args.duration {
            let run_timeout = Duration::from_secs(duration).saturating_add(timeout);
            tokio::select! {
                result = publisher.run(ct.clone()) => {
                    let _outcome = result?;
                }
                _ = tokio::time::sleep(run_timeout) => {
                    ct.cancel();
                    return Err(anyhow!("publisher timed out after {:?}", run_timeout));
                }
                _ = utils::shutdown_signal() => {
                    info!("Shutdown signal received, stopping publisher");
                    ct.cancel();
                }
            }
        } else {
            tokio::select! {
                result = publisher.run(ct.clone()) => {
                    let _outcome = result?;
                }
                _ = utils::shutdown_signal() => {
                    info!("Shutdown signal received, stopping publisher");
                    ct.cancel();
                }
            }
        }
    }

    info!("=== Graceful shutdown completed ===");
    Ok(())
}
