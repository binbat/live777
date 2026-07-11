use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser, ValueEnum};
use tracing::{Level, info};

mod log;

use livetwo::probe::{ProbeBackend, ProbeConfig, ProbeResult, rsmpeg::RsmpegProbe};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
}

#[derive(Parser)]
#[command(name = "whepprobe", version = version::version_with_features!())]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,

    /// WHEP endpoint URL, e.g. http://localhost:7777/whep/live
    #[arg(short, long)]
    whep: String,

    /// Output format: human, json
    #[arg(short, long, value_enum, default_value = "human")]
    output: OutputFormat,

    /// Overall timeout in seconds
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Expected video codec: vp8, vp9, h264, h265, av1
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

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {e:?}");
        std::process::exit(1);
    }
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
        "whepprobe={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

    let config = build_config(&args)?;

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
            print_result(&result, args.output);
            if result.success {
                info!("=== Probe succeeded ===");
                Ok(())
            } else {
                Err(anyhow!(
                    "Probe failed: {}",
                    result.error.as_deref().unwrap_or("unknown error")
                ))
            }
        }
        Ok(Err(e)) => {
            // Emit a structured failure result for JSON consumers, then return
            // a concise error so the process exits non-zero without dumping the
            // same diagnostics twice.
            let failed = ProbeResult::failed("rsmpeg", format!("{e:?}"));
            print_result(&failed, args.output);
            Err(anyhow!("Probe failed"))
        }
        Err(_) => {
            let failed =
                ProbeResult::failed("rsmpeg", format!("timed out after {:?}", config.timeout));
            print_result(&failed, args.output);
            Err(anyhow!("Probe timed out"))
        }
    }
}

fn build_config(args: &Args) -> Result<ProbeConfig> {
    let codec = match &args.codec {
        Some(c) => Some(cli::codec_from_str(c).with_context(|| format!("Invalid codec: {c}"))?),
        None => None,
    };

    Ok(ProbeConfig {
        whep_url: args.whep.clone(),
        timeout: Duration::from_secs(args.timeout),
        codec,
        sprop_params: args.sprop_params.clone(),
        token: args.token.clone(),
    })
}

fn print_result(result: &ProbeResult, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(result)
                .expect("ProbeResult should always be serializable");
            println!("{json}");
        }
        OutputFormat::Human => {
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
