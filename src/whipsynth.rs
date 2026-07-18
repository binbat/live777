//! Legacy alias for `livetwo synth`. New deployments should prefer the unified
//! `livetwo` binary; this thin wrapper keeps existing scripts and packages
//! working unchanged.

use anyhow::Result;
use clap::Parser;

mod livetwo_cli;
mod log;
mod utils;

#[derive(Parser)]
#[command(name = "whipsynth", version = version::version_with_features!())]
struct Args {
    #[command(flatten)]
    inner: livetwo_cli::SynthArgs,
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(e) = livetwo_cli::run_synth("whipsynth", Args::parse().inner).await {
        eprintln!("Error: {e:?}");
        std::process::exit(1);
    }
    Ok(())
}
