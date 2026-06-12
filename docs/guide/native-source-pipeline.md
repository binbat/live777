# Native Source Pipeline

Architecture and build guide for the libcamera / V4L2 / RDK X5 native capture-and-encode pipeline.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│ liveion (RTP / WHEP / source manager)                    │
│                                                          │
│  NativeSource  (unified thin wrapper)                    │
│         │                                                │
│         ▼                                                │
│  NativeEncodedSource                                     │
│    webrtc-rs H264Payloader / Packetizer                  │
│    → MediaPacket::RtpPacket(Arc<Packet>)                 │
│    → inject_rtp (no marshal/unmarshal roundtrip)         │
│         │                                                │
│  RTCP PLI → request_keyframe()                           │
│         │                                                │
│─────────│─ optional dep boundary ────────────────────────│
│         ▼                                                │
│ livesrc (native backend crate)                           │
│                                                          │
│  NativePipeline  (safe Rust wrapper)                     │
│         │                                                │
│         ▼  crate-private FFI                             │
│  native_ffi.rs  →  source_pipeline_ffi.h                 │
│         │                                                │
│         ▼                                                │
│ C++ (libcamera-bridge / cambridge)                       │
│                                                          │
│  SourcePipeline                                          │
│    ├─ CaptureBackend  →  RawFrame  (C++ internal)        │
│    └─ EncoderBackend  →  EncodedPacket                   │
│                              │                           │
│                              ▼  FFI callback             │
│  on_encoded_packet()  ← EncodedPacketFFI                 │
│         │                                                │
│         ▼  data copied immediately → mpsc channel        │
│  EncodedPacket → liveion NativeEncodedSource             │
└──────────────────────────────────────────────────────────┘
```

- **RawFrame** is C++-internal and never crosses the FFI boundary.
- **EncodedPacket** crosses FFI via a pure-C callback inside `livesrc`; data is copied immediately and sent through an mpsc channel to `liveion`.
- All FFI details are crate-private in `livesrc`; `liveion` only sees `EncodedPacket` through the channel.
- **RTP path for native sources**: `EncodedPacket` → webrtc-rs `H264Payloader` / `Packetizer` → `MediaPacket::RtpPacket(Arc<Packet>)` → `track.inject_rtp`.  This avoids the `Packet` → bytes → `Packet::unmarshal` roundtrip that other sources use.
- `MediaPacket::Rtp { data }` bytes path is still used by `rtp_listener` / `rtsp_source` / `sdp_source`.
- **DMA-BUF zero-copy is not yet implemented.**  The `prefer_dmabuf` config field exists in the schema and is plumbed through to the C++ layer.  RDK V4L2 capture exports DMA-BUF fds when `prefer_dmabuf=true`, but `encoder_rdk.cpp` has not yet implemented DMA-BUF fd import — it rejects `BufferKind::DmaBuf`.  The default remains `false`.  Currently all frames are copied through the CPU path.  Full userspace zero-copy requires implementing DMA-BUF import in the RDK encoder backend and handling capture buffer lifetime.

## Config

Native sources are configured under `[[stream.sources]]` in `conf/live777.toml`.
All source configuration goes through `[[stream.sources]]` in `live777.toml`.

### URL-based (non-native: RTSP / SDP / RTP)

```toml
[[stream.sources]]
stream_id = "rtsp_cam"
url = "rtsp://192.168.1.100:554/stream"
```

### Structured native (libcamera / V4L2 / RDK)

```toml
[[stream.sources]]
stream_id = "pi_cam"
kind = "libcamera"

[stream.sources.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 30
pixel_format = "yuv420"

[stream.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.sources.output]
payload_type = 96
clock_rate = 90000
```

`pixel_format` and `codec` values are validated at startup (unknown values error early).  `kind` + `capture` + `encoder` are mutually exclusive with `url`.

`conf/live777.toml` ships with commented-out Pi / RDK examples.  Copy them into your own config to enable a camera source.

### Backend naming

| Layer | Value |
|-------|-------|
| `capture.backend` | `"libcamera"`, `"v4l2"` |
| `encoder.backend` | `"v4l2_m2m"`, `"rdk"` |

### pixel_format values

| TOML string | RawPixelFormat | Numeric |
|---|---|---|
| `yuyv`, `yuyv422` | Yuyv422 | 0 |
| `nv12` | Nv12 | 1 |
| `yuv420`, `yuv420p` | Yuv420p | 2 |
| `mjpeg` | Mjpeg | 3 |
| `rgb888`, `rgb` | Rgb888 | 4 |

### codec values

| TOML string | VideoCodec | Numeric |
|---|---|---|
| `h264` | H264 | 100 |
| `h265`, `hevc` | H265 | 101 |
| `av1` | Av1 | 102 |
| `vp8` | Vp8 | 103 |
| `vp9` | Vp9 | 104 |

## Feature flags

Only platform presets are user-facing.  Each preset includes `native-source` which
implies `source` (autostart) and `dep:livesrc` (native backend).

| Preset | Expands to |
|--------|-----------|
| `native-rpi` | `native-source, livesrc/capture-libcamera, livesrc/capture-v4l2, livesrc/encoder-v4l2-m2m` |
| `native-generic-v4l2` | `native-source, livesrc/capture-v4l2, livesrc/encoder-v4l2-m2m` |
| `native-rdk` | `native-source, livesrc/capture-v4l2, livesrc/encoder-rdk` |

No additional `--features source` is needed — presets include autostart.

```bash
# Raspberry Pi CSI
LIVE777_NATIVE_BACKEND=rpi \
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rpi,webui

# Generic Linux V4L2
LIVE777_NATIVE_BACKEND=generic-v4l2 \
cargo build --bin live777 --release \
  --no-default-features --features native-generic-v4l2,webui

# RDK X5
LIVE777_NATIVE_BACKEND=rdk-x5 \
cargo build --bin live777 --release \
  --no-default-features --features native-rdk,webui
```

## Build

### Prerequisites

- CMake ≥ 3.16
- A C++17 compiler (gcc or clang)
- Platform SDK as needed (libcamera, RDK sysroot)

### Raspberry Pi (libcamera)

```bash
LIVE777_NATIVE_BACKEND=rpi \
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rpi,webui
```

Requires the Pi sysroot with libcamera-dev. Set `PI_SYSROOT` if the sysroot is not at the default path.

### Generic Linux V4L2

```bash
LIVE777_NATIVE_BACKEND=generic-v4l2 \
cargo build --bin live777 --release \
  --no-default-features --features native-generic-v4l2,webui
```

`LIVE777_NATIVE_BACKEND` is **required** when building with `capture-v4l2` without `capture-libcamera`. The build will panic at configure time if it is not set.

### RDK X5

```bash
LIVE777_NATIVE_BACKEND=rdk-x5 \
cargo build --bin live777 --release \
  --no-default-features --features native-rdk,webui
```

Requires the RDK sysroot with `hb_media_codec` libraries. Set `RDK_SYSROOT` if the sysroot is not at the default path.

> **Note:** The DMA-BUF zero-copy encode path is not yet implemented.  See the DMA-BUF notes in the Architecture section above.

### macOS (development / check only)

```bash
cargo check --no-default-features
cargo check --features native-rpi,webui
```

> **Caution:** Do not enable native features on macOS unless the native C++ dependencies (libcamera, V4L2 headers, CMake) are installed. Those features invoke CMake and will fail on a stock macOS system.

## Backend selection (build-time)

The build system **never** infers the backend from `TARGET`.  Selection is explicit, via Cargo presets and `LIVE777_NATIVE_BACKEND`:

| Preset | CMake defines (ON) |
|--------|-------------------|
| `native-rpi` | `ENABLE_BACKEND_PI`, `ENABLE_CAPTURE_LIBCAMERA`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |
| `native-rdk` | `ENABLE_BACKEND_RDK_X5`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RDK_X5` |
| `native-generic-v4l2` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |

When no `capture-*` feature is enabled, CMake is skipped entirely. Encoder-only features do **not** trigger a CMake build — the SourcePipeline requires a capture backend.

## Config examples

### Raspberry Pi CSI

```toml
[[stream.sources]]
stream_id = "pi_cam"
kind = "libcamera"

[stream.sources.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 30
pixel_format = "yuv420"

[stream.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.sources.output]
payload_type = 96
clock_rate = 90000
```

### Raspberry Pi USB V4L2

```toml
[[stream.sources]]
stream_id = "usb_cam"
kind = "v4l2"

[stream.sources.capture]
backend = "v4l2"
device = "/dev/video2"
width = 640
height = 480
fps = 30
pixel_format = "yuyv"

[stream.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.sources.output]
payload_type = 96
clock_rate = 90000
```

### RDK X5

```toml
[[stream.sources]]
stream_id = "rdk_cam"
kind = "v4l2"

[stream.sources.capture]
backend = "v4l2"
device = "/dev/video0"
width = 640
height = 480
fps = 30
pixel_format = "yuyv"

[stream.sources.encoder]
backend = "rdk"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.sources.output]
payload_type = 96
clock_rate = 90000
```

