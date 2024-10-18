use clap::{ArgAction, Parser};
use tracing::Level;

mod log;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
    #[arg(short = 'v', action = ArgAction::Count, default_value_t = 0)]
    verbose: u8,
    /// rtsp://[username]:[password]@[ip]:[port]/[stream] Or <stream.sdp>
    #[arg(short, long, default_value_t = format!("{}://0.0.0.0:8554", livetwo::SCHEME_RTSP_SERVER))]
    input: String,
    /// Set Listener address
    #[arg(long)]
    host: Option<String>,
    /// The WHIP server endpoint to POST SDP offer to. e.g.: https://example.com/whip/777
    #[arg(short, long)]
    whip: String,
    /// Authentication token to use, will be sent in the HTTP Header as 'Bearer '
    #[arg(short, long)]
    token: Option<String>,
    /// Run a command as childprocess
    #[arg(long)]
    command: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    log::set(format!(
        "livetwo={},webrtc=error",
        match args.verbose {
            0 => Level::WARN,
            1 => Level::INFO,
            2 => Level::DEBUG,
            _ => Level::TRACE,
        }
    ));

    livetwo::whip::into(
        args.input.clone(),
        args.host.clone(),
        args.whip.clone(),
        args.token.clone(),
        args.command.clone(),
    )
    .await
    .unwrap();
}
