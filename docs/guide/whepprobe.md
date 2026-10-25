# WhepProbe

`whepprobe` is a diagnostic tool similar to `ffprobe` for WHEP endpoints. It
subscribes to a WHEP stream and verifies that the stream is reachable and that
its media can be decoded with FFmpeg via the `rsmpeg` crate. It decodes VP8,
VP9, H.264, H.265 and AV1 video, plus Opus, G.722, PCMU and PCMA audio. Video-only streams
keep the original success semantics; whenever audio RTP is received, at least
one audio frame must also decode successfully.

For browser-based playback verification, see [`WhepWright`](whepwright).

## Get it

`whepprobe` is included in the all-in-one `live777-all` image:

```bash
docker run --rm --network host ghcr.io/binbat/live777-all:latest \
  whepprobe -w http://localhost:7777/whep/live
```

The image's built-in `smoke-test` command uses `whepprobe` for its WHEP
verification step — see [Getting Started](/guide/getting-started).

## Build

```bash
# Requires FFmpeg development libraries
cargo build --bin whepprobe --features rsmpeg
```

AV1 software decoding requires an FFmpeg build with `libdav1d` or `libaom`
enabled. The probe prefers those decoders over FFmpeg's hardware-oriented
native AV1 decoder.

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
| `--ice-server` | none | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]`. Defaults to none — host candidates only (WHIP/WHEP endpoints advertise their own ICE servers via Link headers) |

## JSON audio fields

Every report contains `audio_frame_count`, `audio_sample_rate` and
`audio_channels`, including failure reports and streams without audio. Their
zero values mean that no audio frame was decoded. For a successfully decoded
track they report the decoded frame count, actual sample rate (G.722 is
16,000 Hz even though its RTP clock is 8,000 Hz) and channel count.

## Exit code

- `0`: WHEP connected and video was decoded; if audio RTP was received, audio was decoded too.
- `1`: probe failed or an error occurred.

## Core library

The probe logic lives in `livetwo::probe` and can be reused by integration tests
or other Rust tools:

```rust
use cli::Codec;
use livetwo::probe::{ProbeBackend, ProbeConfig};
use livetwo::probe::rsmpeg::RsmpegProbe;

// RsmpegProbe requires `livetwo` to be built with the `rsmpeg` feature.
let config = ProbeConfig {
    whep_url: "http://localhost:7777/whep/live".to_string(),
    video_codec: Some(Codec::Vp8),
    ..Default::default()
};

let result = RsmpegProbe::default().probe(&config).await?;
assert!(result.success);
```
