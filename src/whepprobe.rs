//! Legacy alias for `livetwo probe`. New deployments should prefer the unified
//! `livetwo` binary; this thin wrapper keeps existing scripts and packages
//! working unchanged.

use clap::Parser;

mod livetwo_cli;
mod log;
#[allow(dead_code)]
mod utils;

#[derive(Parser)]
#[command(name = "whepprobe", version = version::version_with_features!())]
struct Args {
    #[command(flatten)]
    inner: livetwo_cli::ProbeArgs,
}

#[tokio::main]
async fn main() {
    let code = match livetwo_cli::run_probe("whepprobe", Args::parse().inner).await {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    std::process::exit(code);
}
