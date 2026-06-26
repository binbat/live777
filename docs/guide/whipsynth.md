# WhipSynth

`whipsynth` is a synthetic WHIP publisher. It generates audio and video test
patterns locally with FFmpeg via the `rsmpeg` crate and publishes them to a WHIP
endpoint without any external media source.

## Use cases

- Quickly verify that a WHIP endpoint accepts and forwards streams.
- Load-test a Live777 instance with many concurrent publishers.
- Reproduce codec-specific issues without setting up a real camera or FFmpeg
  pipeline.

## Build

```bash
# Build the whipsynth binary (requires FFmpeg development libraries)
cargo build --bin whipsynth --features rsmpeg
```

## Usage

Publish a 640x480 VP8 video-only stream to a WHIP endpoint:

```bash
whipsynth -w http://localhost:7777/whip/live
```

Publish VP8 video + Opus audio:

```bash
whipsynth -w http://localhost:7777/whip/live --acodec opus
```

Publish H.264 with authentication token and run for 60 seconds:

```bash
whipsynth -w http://localhost:7777/whip/live \
          -t my-token \
          --vcodec h264 \
          --duration 60
```

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-w`, `--whip` | required | WHIP endpoint URL |
| `-t`, `--token` | none | Bearer token for WHIP authentication |
| `--vcodec` | `vp8` | Video codec: `vp8`, `vp9`, `h264`, `h265`, `av1` |
| `--acodec` | none | Audio codec: `opus`, `g722` (omit for no audio) |
| `--width` | `640` | Video width in pixels |
| `--height` | `480` | Video height in pixels |
| `--fps` | `30` | Video frame rate |
| `--duration` | none | Run for the specified number of seconds, then exit |

## Load-test mode

`whipsynth` can spawn multiple concurrent publishers. These options are hidden
from the default help because they are mainly used by the test suite:

```bash
whipsynth -w http://localhost:7777/whip/live --count 10 --spawn-interval-ms 200
```

Each session gets a unique URL by appending an index to the last path segment.
For example, with `--count 3` and base URL `/whip/live`, the sessions publish to
`/whip/live-0`, `/whip/live-1`, and `/whip/live-2`.

| Option | Default | Description |
|--------|---------|-------------|
| `--count` | `1` | Number of concurrent WHIP sessions |
| `--spawn-interval-ms` | `100` | Delay between spawning each session |

## Exit code

- `0`: publishing finished normally (duration elapsed or cancelled).
- `1`: an error occurred, for example the WHIP endpoint rejected the request or
  the peer connection failed.
