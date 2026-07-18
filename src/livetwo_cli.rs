//! Shared CLI implementation for the unified `livetwo` command and the legacy
//! per-tool alias binaries (`whipinto`, `whepfrom`, `whipsynth`, `whepprobe`).
//!
//! All argument parsing and run logic lives here so the unified binary and the
//! aliases can never drift apart. Legacy flag spellings (`--whip`, `--whep`)
//! are kept as visible aliases of the unified `--url` spelling.

// Each alias binary uses only the subcommand it wraps; the rest of this shared
// module is intentionally compiled but unused there.
#![allow(dead_code)]

#[cfg(feature = "rsmpeg")]
use std::time::Duration;

use anyhow::Result;
#[cfg(feature = "rsmpeg")]
use anyhow::{Context, anyhow};
#[cfg(feature = "rsmpeg")]
use clap::ValueEnum;
use clap::{ArgAction, Args, Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing::{Level, info};

use crate::{log, utils};

/// Unified `livetwo` command line: all protocol-conversion tools in one binary.
#[derive(Parser)]
#[command(
    name = "livetwo",
    version = version::version_with_features!(),
    about = "WHIP/WHEP <-> RTP/RTSP protocol converter"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Publish a stream into a WHIP endpoint (formerly whipinto)
    Whip(WhipArgs),
    /// Pull a stream from a WHEP endpoint (formerly whepfrom)
    Whep(WhepArgs),
    /// Publish in-process generated test frames (formerly whipsynth)
    #[cfg(feature = "rsmpeg")]
    Synth(SynthArgs),
    /// Probe a WHEP endpoint and decode a few frames (formerly whepprobe)
    #[cfg(feature = "rsmpeg")]
    Probe(ProbeArgs),
}

/// Entry point of the unified `livetwo` binary.
pub async fn cli_main() {
    let code = match run_cli().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    std::process::exit(code);
}

async fn run_cli() -> Result<i32> {
    match Cli::parse().command {
        Commands::Whip(args) => run_whip("livetwo", args).await.map(|_| 0),
        Commands::Whep(args) => run_whep("livetwo", args).await.map(|_| 0),
        #[cfg(feature = "rsmpeg")]
        Commands::Synth(args) => run_synth("livetwo", args).await.map(|_| 0),
        #[cfg(feature = "rsmpeg")]
        Commands::Probe(args) => run_probe("livetwo", args)
            .await
            .map(|success| i32::from(!success)),
    }
}

#[derive(Debug, Args)]
pub struct WhipArgs {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// rtp://[ip]:[port] / rtsp://[username]:[password]@[ip]:[port]/[stream] / <stream.sdp> / synth://<vcodec>?...
    #[arg(short, long, default_value_t = format!("{}://0.0.0.0:8554", livetwo::SCHEME_RTP_SDP))]
    input: String,
    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
    #[arg(short = 'w', long = "url", visible_alias = "whip")]
    url: String,
    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,
    /// Run a command as childprocess
    #[arg(long)]
    command: Option<String>,
}

#[derive(Debug, Args)]
pub struct WhepArgs {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// rtp://[ip]:[port] / rtsp-listen://[ip]:[port] / rtsp://[username]:[password]@[ip]:[port]/[stream] / <output.sdp>
    #[arg(short, long, default_value_t = format!("{}://0.0.0.0:8555", livetwo::SCHEME_RTP_SDP))]
    output: String,
    /// The WHEP server endpoint to POST SDP offer to. e.g.: https://example.com/whep/777
    #[arg(short = 'u', long = "url", visible_alias = "whep")]
    url: String,
    /// SDP filename to write to (used in RTP mode)
    #[arg(long, default_value = "output.sdp")]
    sdp_file: Option<String>,
    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,
    /// Run a command as childprocess
    #[arg(long)]
    command: Option<String>,
    /// Channel URL for DataChannel <-> UDP forwarding
    /// Format: udp://<listen_host>:<listen_port>?host=<target_host>&port=<target_port>
    /// Example: udp://0.0.0.0:9001?host=127.0.0.1&port=9000
    #[arg(long)]
    channel: Option<String>,
}

#[cfg(feature = "rsmpeg")]
#[derive(Debug, Args)]
pub struct SynthArgs {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
    #[arg(short = 'w', long = "url", visible_alias = "whip")]
    url: String,
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

#[cfg(feature = "rsmpeg")]
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProbeOutputFormat {
    Human,
    Json,
}

#[cfg(feature = "rsmpeg")]
#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// WHEP endpoint URL, e.g. http://localhost:7777/whep/live
    #[arg(short = 'u', long = "url", visible_alias = "whep")]
    url: String,
    /// Output format: human, json
    #[arg(short, long, value_enum, default_value = "human")]
    output: ProbeOutputFormat,
    /// Overall timeout in seconds
    #[arg(long, default_value_t = 30)]
    timeout: u64,
    /// Expected video codec: vp8, vp9, h264, h265, av1.
    /// The rsmpeg backend auto-detects the codec from the WHEP session, so
    /// this option only affects the reported result.
    #[arg(long)]
    codec: Option<String>,
    /// H265 sprop parameters (`sprop-vps=...;sprop-sps=...;sprop-pps=...`)
    #[arg(long)]
    sprop_params: Option<String>,
    /// How many seconds to decode after the WHEP session connects
    #[arg(long, default_value_t = 5)]
    decode_duration: u64,
    /// Authentication token to use, sent in the HTTP Authorization header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,
}

fn simple_log_level(verbose: u8) -> Level {
    match verbose {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    }
}

fn detailed_log_levels(verbose: u8) -> (Level, Level) {
    match verbose {
        0 => (Level::WARN, Level::ERROR),
        1 => (Level::INFO, Level::WARN),
        2 => (Level::DEBUG, Level::INFO),
        _ => (Level::TRACE, Level::DEBUG),
    }
}

/// Publish a stream into a WHIP endpoint (the former `whipinto` flow).
pub async fn run_whip(tool: &'static str, args: WhipArgs) -> Result<()> {
    let log_level = simple_log_level(args.verbose);
    log::set(format!(
        "{tool}={},livetwo={},rtsp={},webrtc=error",
        log_level, log_level, log_level,
    ));

    let ct = CancellationToken::new();
    let handle = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        args.input.clone(),
        args.url.clone(),
        args.token.clone(),
        args.command.clone(),
    ));

    utils::shutdown_signal().await;
    ct.cancel();
    handle.await??;
    info!("=== Graceful shutdown completed ===");

    Ok(())
}

/// Pull a stream from a WHEP endpoint (the former `whepfrom` flow).
pub async fn run_whep(tool: &'static str, args: WhepArgs) -> Result<()> {
    let log_level = simple_log_level(args.verbose);
    log::set(format!(
        "{tool}={},livetwo={},rtsp={},webrtc=error",
        log_level, log_level, log_level,
    ));

    let ct = CancellationToken::new();
    let handle = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        args.output.clone(),
        args.url.clone(),
        args.sdp_file.clone(),
        args.token.clone(),
        args.command.clone(),
        args.channel.clone(),
    ));

    utils::shutdown_signal().await;
    ct.cancel();
    handle.await??;
    info!("=== Graceful shutdown completed ===");

    Ok(())
}

/// Publish in-process generated test frames (the former `whipsynth` flow).
#[cfg(feature = "rsmpeg")]
pub async fn run_synth(tool: &'static str, args: SynthArgs) -> Result<()> {
    let (log_level, webrtc_level) = detailed_log_levels(args.verbose);
    log::set(format!(
        "{tool}={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

    let video_codec_cli = cli::codec_from_str(&args.video_codec)
        .with_context(|| format!("Invalid video codec: {}", args.video_codec))?;
    let video_codec = livetwo::source::VideoCodec::from_cli(video_codec_cli)
        .ok_or_else(|| anyhow!("Unsupported video codec: {}", args.video_codec))?;

    let audio_codec = match &args.audio_codec {
        Some(name) => {
            let audio_codec_cli = cli::codec_from_str(name)
                .with_context(|| format!("Invalid audio codec: {name}"))?;
            Some(
                livetwo::source::AudioCodec::from_cli(audio_codec_cli)
                    .ok_or_else(|| anyhow!("Unsupported audio codec: {name}"))?,
            )
        }
        None => None,
    };

    let config = livetwo::whipsynth::PublisherConfig {
        whip_url: args.url.clone(),
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
        whip_url = %args.url,
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
                    let _stats = result?;
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
                    let _stats = result?;
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

/// Probe a WHEP endpoint (the former `whepprobe` flow).
///
/// Returns `Ok(true)` when the probe succeeded, `Ok(false)` for a completed
/// but unsuccessful probe, and `Err` for CLI-level failures.
#[cfg(feature = "rsmpeg")]
pub async fn run_probe(tool: &'static str, args: ProbeArgs) -> Result<bool> {
    use livetwo::probe::{ProbeBackend, ProbeConfig, ProbeResult, rsmpeg::RsmpegProbe};

    let (log_level, webrtc_level) = detailed_log_levels(args.verbose);
    log::set(format!(
        "{tool}={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

    let codec = match &args.codec {
        Some(c) => Some(cli::codec_from_str(c).with_context(|| format!("Invalid codec: {c}"))?),
        None => None,
    };

    let config = ProbeConfig {
        whep_url: args.url.clone(),
        timeout: Duration::from_secs(args.timeout),
        codec,
        sprop_params: args.sprop_params.clone(),
        token: args.token.clone(),
    };

    info!(
        whep_url = %config.whep_url,
        codec = ?config.codec,
        timeout = ?config.timeout,
        "Starting WHEP probe (rsmpeg)"
    );

    let backend = RsmpegProbe {
        decode_duration: Duration::from_secs(args.decode_duration),
    };

    let result = tokio::time::timeout(config.timeout, backend.probe(&config)).await;

    match result {
        Ok(Ok(result)) => {
            let success = result.success;
            print_probe_result(&result, args.output);
            if success {
                info!("=== Probe succeeded ===");
            }
            Ok(success)
        }
        Ok(Err(e)) => {
            // Emit a structured failure result for JSON consumers without
            // duplicating the diagnostics on stderr.
            let failed = ProbeResult::failed("rsmpeg", format!("{e:?}"));
            print_probe_result(&failed, args.output);
            Ok(false)
        }
        Err(_) => {
            let failed =
                ProbeResult::failed("rsmpeg", format!("timed out after {:?}", config.timeout));
            print_probe_result(&failed, args.output);
            Ok(false)
        }
    }
}

#[cfg(feature = "rsmpeg")]
fn print_probe_result(result: &livetwo::probe::ProbeResult, format: ProbeOutputFormat) {
    match format {
        ProbeOutputFormat::Json => {
            let json = serde_json::to_string_pretty(result)
                .expect("ProbeResult should always be serializable");
            println!("{json}");
        }
        ProbeOutputFormat::Human => {
            println!("=== WHEP Probe Result ===");
            println!("Backend:        {}", result.backend);
            println!("Connected:      {}", result.connected);
            println!("Success:        {}", result.success);
            if let Some(codec) = &result.codec {
                println!("Codec:          {}", codec);
            }
            if result.width > 0 && result.height > 0 {
                println!("Resolution:     {}x{}", result.width, result.height);
            }
            if result.frame_count > 0 {
                println!("Frames decoded: {}", result.frame_count);
            }
            if result.video_tracks > 0 {
                println!("Video tracks:   {}", result.video_tracks);
            }
            if result.audio_tracks > 0 {
                println!("Audio tracks:   {}", result.audio_tracks);
            }
            if result.video_bytes_received > 0 {
                println!("Video bytes:    {}", result.video_bytes_received);
            }
            if result.audio_bytes_received > 0 {
                println!("Audio bytes:    {}", result.audio_bytes_received);
            }
            println!("Duration:       {} ms", result.duration_ms);
            if let Some(error) = &result.error {
                println!("Error:          {}", error);
            }
        }
    }
}
