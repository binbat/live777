//! Legacy alias for `livetwo whep`. New deployments should prefer the unified
//! `livetwo` binary; this thin wrapper keeps existing scripts and packages
//! working unchanged.

use anyhow::Result;
use clap::Parser;

mod livetwo_cli;
mod log;
mod utils;

#[derive(Parser)]
#[command(name = "whepfrom", version = version::version_with_features!())]
struct Args {
    #[command(flatten)]
    inner: livetwo_cli::WhepArgs,
}

#[tokio::main]
async fn main() -> Result<()> {
    livetwo_cli::run_whep("whepfrom", Args::parse().inner).await
}
