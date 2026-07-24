# WhepProbe

`whepprobe` is a diagnostic tool similar to `ffprobe` for WHEP endpoints. It
subscribes to a WHEP stream and verifies that the stream is reachable and that
video can be decoded with FFmpeg via the `rsmpeg` crate.

For browser-based playback verification, see [`WhepWright`](whepwright).

## Build

```bash
# Requires FFmpeg development libraries
cargo build --bin whepprobe --features rsmpeg
```

## Usage

Probe a WHEP endpoint:

```bash
whepprobe -w http://localhost:7777/whep/live
```

Specify the expected codec and timeout:

```bash
whepprobe -w http://localhost:7777/whep/live --codec h264 --timeout 60
```

Get JSON output for scripts or CI:

```bash
whepprobe -w http://localhost:7777/whep/live --output json
```

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-w`, `--whep` | required | WHEP endpoint URL |
| `-t`, `--token` | none | Bearer token for WHEP authentication |
| `-v` | `warn` | Increase verbosity (`-v` info, `-vv` debug, `-vvv` trace) |
| `--codec` | auto-detect | Expected video codec: `vp8`, `vp9`, `h264`, `h265`, `av1`. The `rsmpeg` backend auto-detects the codec from the WHEP session, so this option only affects the reported result. |
| `--sprop-params` | none | H.265 sprop parameters (`sprop-vps=...;sprop-sps=...;sprop-pps=...`) |
| `--decode-duration` | `5` | Seconds to decode after the WHEP session connects. Values above `10` are silently capped. |
| `--output` | `human` | Output format: `human`, `json` |
| `--timeout` | `30` | Overall timeout in seconds |

## Exit code

- `0`: probe succeeded (WHEP connected and video was decoded).
- `1`: probe failed or an error occurred.

## Core library

The probe logic lives in `livetwo::probe` and can be reused by integration tests
or other Rust tools:

```rust
use cli::Codec;
use livetwo::probe::{ProbeBackend, ProbeConfig};
use livetwo::probe::rsmpeg::RsmpegProbe;
use std::time::Duration;

// RsmpegProbe requires `livetwo` to be built with the `rsmpeg` feature.
let config = ProbeConfig {
    whep_url: "http://localhost:7777/whep/live".to_string(),
    video_codec: Some(Codec::Vp8),
    ..Default::default()
};

let result = RsmpegProbe::default().probe(&config).await?;
assert!(result.success);
```
