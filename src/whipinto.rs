use anyhow::Result;
use clap::{ArgAction, Parser};
use tokio_util::sync::CancellationToken;
use tracing::{Level, info};

mod log;
mod utils;

#[derive(Parser)]
#[command(name = "whipinto", version = version::version_with_features!())]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// rtp://[ip]:[port] / rtsp://[username]:[password]@[ip]:[port]/[stream] / <stream.sdp>
    #[arg(short, long, default_value_t = format!("{}://0.0.0.0:8554", livetwo::SCHEME_RTP_SDP))]
    input: String,
    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
    #[arg(short, long)]
    whip: String,
    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,
    /// Run a command as childprocess
    #[arg(long)]
    command: Option<String>,
    /// ICE server used for offer gathering, repeatable; format
    /// `<url>[,<username>[,<credential>]]`. Pass an empty string to use host
    /// candidates only.
    #[arg(long = "ice-server", value_name = "SPEC", default_value = iceserver::DEFAULT_ICE_SERVER_URL)]
    ice_servers: Vec<iceserver::IceServer>,
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
        "whipinto={},livetwo={},rtsp={},webrtc=error",
        log_level, log_level, log_level,
    ));

    let ct = CancellationToken::new();
    let handle = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        args.input.clone(),
        args.whip.clone(),
        args.token.clone(),
        args.command.clone(),
        iceserver::to_rtc_ice_servers(args.ice_servers),
    ));

    utils::shutdown_signal().await;
    ct.cancel();
    handle.await??;
    info!("=== Graceful shutdown completed ===");

    Ok(())
}
