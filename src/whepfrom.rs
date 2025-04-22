use clap::{ArgAction, Parser};
use tracing::Level;

mod log;

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

    livetwo::whep::from(
        args.output.clone(),
        args.whep.clone(),
        args.token.clone(),
        args.command.clone(),
    )
    .await
    .unwrap();
}
