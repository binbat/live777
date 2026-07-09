use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser, ValueEnum};
use serde::Serialize;
use tracing::{Level, info};

mod log;

use playwright_whep::{Browser, HarnessResult, WhepBrowserPlayer};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BrowserArg {
    Chromium,
    Firefox,
    Webkit,
}

#[derive(Parser)]
#[command(name = "whepwright", version = version::version_with_features!())]
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

    /// Browser to use
    #[arg(long, value_enum, default_value = "chromium")]
    browser: BrowserArg,

    /// Browser channel to use (e.g. `chrome` or `msedge` for Chromium)
    #[arg(long)]
    channel: Option<String>,

    /// Run the browser in headless mode [default: true]
    #[arg(long, value_parser = clap::builder::BoolishValueParser::new())]
    headless: Option<bool>,

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
        "whepwright={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    ));

    let headless = args.headless.unwrap_or(true);
    info!(
        whep_url = %args.whep,
        browser = ?args.browser,
        headless = headless,
        timeout = args.timeout,
        "Starting WHEP browser playback"
    );

    let result = probe_playwright(&args, headless).await;

    match result {
        Ok(result) => {
            if let Err(e) = print_result(&result, args.output) {
                eprintln!("Failed to serialize result: {e}");
                return Err(e);
            }
            if result.success {
                info!("=== WHEP playback succeeded ===");
                Ok(())
            } else {
                Err(anyhow!(
                    "WHEP playback failed: {}",
                    result.error.as_deref().unwrap_or("unknown error")
                ))
            }
        }
        Err(e) => {
            let failed = PlayResult::failed(format!("{e:?}"));
            let _ = print_result(&failed, args.output);
            Err(e)
        }
    }
}

async fn probe_playwright(args: &Args, headless: bool) -> Result<PlayResult> {
    let browser = match args.browser {
        BrowserArg::Chromium => Browser::Chromium,
        BrowserArg::Firefox => Browser::Firefox,
        BrowserArg::Webkit => Browser::Webkit,
    };

    let mut player = WhepBrowserPlayer::new(&args.whep)
        .browser(browser)
        .timeout(Duration::from_secs(args.timeout))
        .headless(headless)
        .token(args.token.clone().unwrap_or_default());
    if let Some(channel) = &args.channel {
        player = player.channel(channel.clone());
    }
    let result = player
        .play()
        .await
        .context("Playwright WHEP playback failed")?;

    let subscribe = match result {
        HarnessResult::Subscribe(r) => r,
        HarnessResult::Both(r) => r.subscribe.context("missing subscribe result")?,
        HarnessResult::Publish(_) => {
            return Err(anyhow::anyhow!("expected subscribe result, got publish"));
        }
    };

    Ok(PlayResult {
        success: subscribe.success && subscribe.connected && subscribe.video_width > 0,
        connected: subscribe.connected,
        width: subscribe.video_width,
        height: subscribe.video_height,
        duration_ms: subscribe.duration_ms,
        video_tracks: subscribe.video_tracks as u32,
        audio_tracks: subscribe.audio_tracks as u32,
        video_bytes_received: subscribe.video_bytes_received,
        audio_bytes_received: subscribe.audio_bytes_received,
        error: subscribe.error,
    })
}

#[derive(Debug, Clone, Serialize)]
struct PlayResult {
    success: bool,
    connected: bool,
    width: u32,
    height: u32,
    duration_ms: u64,
    video_tracks: u32,
    audio_tracks: u32,
    video_bytes_received: u64,
    audio_bytes_received: u64,
    error: Option<String>,
}

impl PlayResult {
    fn failed(error: impl Into<String>) -> Self {
        Self {
            success: false,
            connected: false,
            width: 0,
            height: 0,
            duration_ms: 0,
            video_tracks: 0,
            audio_tracks: 0,
            video_bytes_received: 0,
            audio_bytes_received: 0,
            error: Some(error.into()),
        }
    }
}

fn print_result(result: &PlayResult, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let json =
                serde_json::to_string_pretty(result).context("Failed to serialize result")?;
            println!("{json}");
        }
        OutputFormat::Human => {
            println!("=== WHEP Play Result ===");
            println!("Connected:      {}", result.connected);
            println!("Success:        {}", result.success);
            if result.width > 0 && result.height > 0 {
                println!("Resolution:     {}x{}", result.width, result.height);
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
    Ok(())
}
