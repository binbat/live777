//! Unified `livetwo` command: all protocol-conversion tools in one binary.
//!
//! Subcommands:
//! - `livetwo whip`  — publish a stream into a WHIP endpoint (alias: whipinto)
//! - `livetwo whep`  — pull a stream from a WHEP endpoint (alias: whepfrom)
//! - `livetwo synth` — publish generated test frames (alias: whipsynth, rsmpeg)
//! - `livetwo probe` — probe/decode a WHEP endpoint (alias: whepprobe, rsmpeg)

mod livetwo_cli;
mod log;
mod utils;

#[tokio::main]
async fn main() {
    livetwo_cli::cli_main().await;
}
