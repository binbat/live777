use anyhow::Result;
use clap::{ArgAction, Parser};
use tokio_util::sync::CancellationToken;
use tracing::{Level, info};

mod log;
mod utils;

#[derive(Parser)]
#[command(name = "whepfrom", version)]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// rtsp://[username]:[password]@[ip]:[port]/[stream] Or <stream.sdp>
    #[arg(short, long, default_value_t = format!("{}://0.0.0.0:8555", livetwo::SCHEME_RTSP_SERVER))]
    output: String,
    /// The WHEP server endpoint to POST SDP offer to. e.g.: https://example.com/whep/777
    #[arg(short, long)]
    whep: String,
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = match args.verbose {
        0 => Level::WARN,
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    };

    log::set(format!(
        "whepfrom={},livetwo={},rtsp={},webrtc=error",
        log_level, log_level, log_level,
    ));

    let ct = CancellationToken::new();
    let handle = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        args.output.clone(),
        args.whep.clone(),
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
