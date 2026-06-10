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
│    Annex-B parse → SPS profile detect                    │
│    → H264 packetize (RFC 6184) → RTP broadcast           │
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
- **DMA-BUF** fds do not cross FFI; the zero-copy path (WIP) stays within C++.

## Config paths

Two config formats coexist in `conf/live777.toml`:

### Path A: Legacy URL (backward-compatible, non-native only)

```toml
[[stream.sources]]
stream_id = "rtsp_cam"
url = "rtsp://192.168.1.100:554/stream"
```

URL-based config is supported for RTSP, SDP, and RTP sources.
Native sources (libcamera, V4L2) must use the structured format in Path B.

### Path B: Structured native (recommended)

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

This path maps directly to `NativeEncodedSource` — no legacy URL roundtrip.
`pixel_format` and `codec` values are validated at startup (unknown values error early).

### Backend naming

| Layer | Canonical value | Legacy aliases (still accepted) |
|-------|----------------|-------------------------------|
| `capture.backend` | `"v4l2"`, `"libcamera"` | `"rdk-x5"`, `"rdk_x5"` → `"v4l2"` |
| `encoder.backend` | `"v4l2-m2m"`, `"rdk"` | `"v4l2_m2m"` → `"v4l2-m2m"`, `"rdk_x5"` → `"rdk"` |

Legacy values are normalized in the C++ `backend_factory.cpp` dispatcher.

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

The feature system is split into image-source (`livesrc-*`) and encoder (`liveenc-*`) layers:

| Cargo feature | Enables |
|---|---|
| `livesrc-libcamera` | libcamera capture backend |
| `livesrc-v4l2` | V4L2 capture backend |
| `liveenc-v4l2-m2m` | V4L2 M2M hardware encoder |
| `liveenc-rdk` | RDK X5 hardware encoder |
| `webui` | Embedded web frontend |

Platform presets (convenience combinations):

| Preset | Expands to |
|--------|-----------|
| `native-rpi` | `native-source, livesrc/capture-libcamera, livesrc/capture-v4l2, livesrc/encoder-v4l2-m2m` |
| `native-generic-v4l2` | `native-source, livesrc/capture-v4l2, livesrc/encoder-v4l2-m2m` |
| `native-rdk` | `native-source, livesrc/capture-v4l2, livesrc/encoder-rdk` |

Always use a **preset** for a runnable pipeline:

```bash
cargo build --features native-rpi
cargo build --features native-generic-v4l2
cargo build --features native-rdk
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
  --target armv7-unknown-linux-gnueabihf \
  --features native-rpi,webui
```

Requires the Pi sysroot with libcamera-dev. Set `PI_SYSROOT` if the sysroot
is not at the default path.

### Generic Linux V4L2 (no libcamera, no RDK)

```bash
LIVE777_NATIVE_BACKEND=generic-v4l2 \
cargo build --bin live777 --release \
  --features native-generic-v4l2,webui
```

`LIVE777_NATIVE_BACKEND` is **required** when building with only `livesrc-v4l2`.
The build will panic at configure time if it is not set.

### RDK X5

```bash
LIVE777_NATIVE_BACKEND=rdk-x5 \
cargo build --bin live777 --release \
  --features native-rdk,webui
```

Requires the RDK sysroot with `hb_media_codec` libraries.
Set `RDK_SYSROOT` if the sysroot is not at the default path.

> **Note:** The DMA-BUF zero-copy encode path (`prefer_dmabuf = true`) is
> still WIP. Use the CPU copy path (default) for production.

### macOS (development / check only)

CMake native builds are skipped when no native source features are active:

```bash
cargo check --no-default-features
cargo check --features native-rpi
```

> **Caution:** Do not enable `livesrc-*` or `liveenc-*` features on macOS
> unless the native C++ dependencies (libcamera, V4L2 headers, CMake) are
> installed. Those features invoke CMake and will fail on a stock macOS
> system.

## Backend selection (build-time)

The build system **never** infers the backend from `TARGET`.  Selection is
explicit, via Cargo features and the `LIVE777_NATIVE_BACKEND` environment
variable.  CMake options are driven by feature flags, not hardcoded per
platform:

| Feature | CMake define |
|---------|-------------|
| `livesrc-libcamera` | `ENABLE_CAPTURE_LIBCAMERA` |
| `livesrc-v4l2` | `ENABLE_CAPTURE_V4L2` |
| `liveenc-v4l2-m2m` | `ENABLE_ENCODER_V4L2_M2M` |
| `liveenc-rdk` | `ENABLE_ENCODER_RDK_X5` |

`LIVE777_NATIVE_BACKEND` only selects sysroot paths and platform-specific
link libraries (`rpi`, `rdk-x5`, `generic-v4l2`).

When no `livesrc-*` feature is enabled, CMake is skipped entirely.
Encoder-only features (`liveenc-*` without `livesrc-*`) do **not** trigger a
CMake build — the SourcePipeline requires a capture backend.

## What was NOT removed

- `legacy_url.rs` has been removed. Native sources only support structured config.
- RTSP / SDP / RTP URL-based sources remain fully functional (used by non-native sources).
- Old bridge files and legacy C ABI wrappers removed in PR6A/6B cleanup.
