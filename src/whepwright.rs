use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser, ValueEnum};
use serde::Serialize;
use tracing::{Level, info};

mod log;

use playwright_whep::{
    Browser, DiagnosticArtifacts, DiagnosticsOptions, HarnessResult, WhepBrowserPlayer,
};

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

    /// Parent directory for per-run diagnostics
    #[arg(long, default_value = "whepwright-diagnostics")]
    diagnostics_dir: PathBuf,

    /// Capture a Playwright trace and retain it as trace.zip on failure
    #[arg(long, action = ArgAction::SetTrue)]
    trace: bool,
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<bool> {
    let args = Args::parse();

    let (log_level, webrtc_level) = match args.verbose {
        0 => (Level::WARN, Level::ERROR),
        1 => (Level::INFO, Level::WARN),
        2 => (Level::DEBUG, Level::INFO),
        _ => (Level::TRACE, Level::DEBUG),
    };

    let filter = format!(
        "whepwright={},livetwo={},webrtc={}",
        log_level, log_level, webrtc_level,
    );
    if matches!(args.output, OutputFormat::Json) {
        log::set_stderr(filter);
    } else {
        log::set(filter);
    }

    let headless = args.headless.unwrap_or(true);
    info!(
        whep_url = %args.whep,
        browser = ?args.browser,
        headless = headless,
        timeout = args.timeout,
        "Starting WHEP browser playback"
    );

    let run_directory = unique_run_directory(&args.diagnostics_dir)?;
    let result = match probe_playwright(&args, headless, run_directory.clone()).await {
        Ok(result) => result,
        Err(error) => {
            let message = format!("{error:?}");
            let mut artifacts = DiagnosticArtifacts::discover(run_directory).await;
            if message.contains("timed out")
                && !artifacts
                    .capture_errors
                    .iter()
                    .any(|error| error.contains("timed out"))
            {
                artifacts.capture_errors.push(
                    "runner timed out; diagnostics may be incomplete and remaining runner processes were killed"
                        .to_string(),
                );
            }
            PlayResult::failed(message, artifacts)
        }
    };
    let success = result.success;
    print_result(&result, args.output)?;
    if success {
        info!("=== WHEP playback succeeded ===");
    }
    Ok(success)
}

fn unique_run_directory(parent: &Path) -> Result<PathBuf> {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let directory = parent.join(format!("run-{timestamp}-{}-{sequence}", std::process::id()));
    if directory.is_absolute() {
        Ok(directory)
    } else {
        Ok(std::env::current_dir()?.join(directory))
    }
}

async fn probe_playwright(
    args: &Args,
    headless: bool,
    run_directory: PathBuf,
) -> Result<PlayResult> {
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
    let run = player
        .play_with_diagnostics(DiagnosticsOptions {
            directory: run_directory,
            trace: args.trace,
        })
        .await
        .context("Playwright WHEP playback failed")?;

    let subscribe = match run.result {
        HarnessResult::Subscribe(r) => r,
        HarnessResult::Both(r) => r.subscribe.context("missing subscribe result")?,
        HarnessResult::Publish(_) => {
            return Err(anyhow!("expected subscribe result, got publish"));
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
        artifacts: run.artifacts,
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
    artifacts: DiagnosticArtifacts,
}

impl PlayResult {
    fn failed(error: impl Into<String>, artifacts: DiagnosticArtifacts) -> Self {
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
            artifacts,
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
            if result.artifacts.retained {
                println!("Diagnostics:    {}", result.artifacts.directory.display());
            }
            for error in &result.artifacts.capture_errors {
                println!("Capture error:  {error}");
            }
        }
    }
    Ok(())
}
