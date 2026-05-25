# Native Source Pipeline

Architecture and build guide for the libcamera / V4L2 / RDK X5 native capture-and-encode pipeline.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Rust (liveion)                                          │
│                                                         │
│  LibcameraSource / V4L2Source  (thin wrappers)          │
│         │                                               │
│         ▼                                               │
│  NativeEncodedSource  (shared impl)                     │
│         │                                               │
│         ▼  pure-C FFI                                   │
│  SourcePipeline FFI  (source_pipeline_ffi.h)            │
│         │                                               │
│         ▼                                               │
│  C++ (libcamera-bridge / cambridge)                     │
│                                                         │
│  SourcePipeline                                         │
│    ├─ CaptureBackend  →  RawFrame  (C++ internal)       │
│    └─ EncoderBackend  →  EncodedPacket                  │
│                              │                          │
│                              ▼  FFI callback            │
│  on_encoded_packet()  ← EncodedPacketFFI                │
│         │                                               │
│         ▼  data copied immediately                      │
│  Annex-B parse → SPS profile detect                     │
│  → H264 packetize (RFC 6184) → RTP broadcast            │
│                                                         │
│  RTCP PLI → request_keyframe() → FFI → C++ encoder      │
└─────────────────────────────────────────────────────────┘
```

- **RawFrame** is C++-internal and never crosses the FFI boundary.
- **EncodedPacket** crosses FFI via a pure-C callback; Rust copies data immediately.
- **DMA-BUF** fds do not cross FFI; the zero-copy path (WIP) stays within C++.

## Config paths

Two config formats coexist in `conf/live777.toml`:

### Path A: Legacy URL (backward-compatible)

```toml
[stream]
[[stream.sources]]
stream_id = "pi_cam"
url = "libcamera://0?width=640&height=480&fps=30"
```

Parameters are embedded in the URL query string. This path remains supported.

### Path B: Structured native (recommended)

```toml
[stream]
[[stream.sources_v2]]
stream_id = "pi_cam"
kind = "libcamera"

[stream.sources_v2.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 30
pixel_format = "yuv420"

[stream.sources_v2.encoder]
backend = "v4l2_m2m"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.sources_v2.output]
payload_type = 96
clock_rate = 90000
```

This path maps directly to `NativeEncodedSource` — no legacy URL roundtrip.
`pixel_format` and `codec` values are validated at startup (unknown values error early).

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

## Build

### Prerequisites

- CMake ≥ 3.16
- A C++17 compiler (gcc or clang)
- Platform SDK as needed (libcamera, RDK sysroot)

### Feature flags

| Cargo feature | Enables |
|---|---|
| `source-libcamera` | libcamera capture (Pi) |
| `source-v4l2` | V4L2 direct capture |
| `backend-rdk-x5` | RDK X5 platform paths |
| `encoder-rdk-x5` | RDK X5 hardware encoder (implies `backend-rdk-x5`) |
| `webui` | Embedded web frontend |

### Raspberry Pi (libcamera)

```bash
LIVE777_NATIVE_BACKEND=rpi \
cargo build --bin live777 --release \
  --target armv7-unknown-linux-gnueabihf \
  --features source-libcamera,webui
```

Requires the Pi sysroot with libcamera-dev. Set `PI_SYSROOT` if the sysroot
is not at the default path.

### Generic Linux V4L2 (no libcamera, no RDK)

```bash
LIVE777_NATIVE_BACKEND=generic-v4l2 \
cargo build --bin live777 --release \
  --features source-v4l2,webui
```

`LIVE777_NATIVE_BACKEND` is **required** when building with only `source-v4l2`.
The build will panic at configure time if it is not set.

### RDK X5

```bash
LIVE777_NATIVE_BACKEND=rdk-x5 \
cargo build --bin live777 --release \
  --features source-v4l2,backend-rdk-x5,encoder-rdk-x5,webui
```

Requires the RDK sysroot with `hb_media_codec` libraries.
Set `RDK_SYSROOT` if the sysroot is not at the default path.

> **Note:** The DMA-BUF zero-copy encode path (`prefer_dmabuf = true`) is
> still WIP. Use the CPU copy path (default) for production.

### macOS (development / check only)

CMake native builds are skipped when no native source features are active:

```bash
cargo check --no-default-features
cargo check -p liveion --features source
```

> **Caution:** Do not enable `source-libcamera` or `source-v4l2` on macOS
> unless the native C++ dependencies (libcamera, V4L2 headers, CMake) are
> installed.  Those features invoke CMake and will fail on a stock macOS
> system.

## Backend selection (build-time)

The build system **never** infers the backend from `TARGET`.  Selection is
explicit, via Cargo features and the `LIVE777_NATIVE_BACKEND` environment
variable:

| `LIVE777_NATIVE_BACKEND` | CMake defines (ON) |
|---|---|
| `rpi` | `ENABLE_BACKEND_PI`, `ENABLE_CAPTURE_LIBCAMERA`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |
| `rdk-x5` | `ENABLE_BACKEND_RDK_X5`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RDK_X5` |
| `generic-v4l2` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` | 

When no native source feature is enabled (`source-libcamera`, `source-v4l2`,
`backend-rdk-x5`), CMake is skipped entirely.

## Commit series (for reviewers)

| PR | Title | Summary |
|----|-------|---------|
| PR1A | Feature & build gate cleanup | Decoupled `source-v4l2` from `source-libcamera`; explicit `LIVE777_NATIVE_BACKEND`; no more `aarch64` inference |
| PR1B | Structured source config | `SourceSpec` / `CaptureSpec` / `EncoderSpec` / `OutputSpec` with serde; legacy URL fallback |
| PR2 | CaptureBackend abstraction | `RawFrame` + `CaptureBackend` (pure C++); libcamera and V4L2 backends |
| PR3 | EncoderBackend abstraction | `EncodedPacket` + `EncoderBackend` (pure C++); V4L2 M2M and RDK X5 backends |
| PR4A | C++ SourcePipeline FFI | Pure-C ABI (`source_pipeline_ffi.h`); `SourcePipeline` class connecting capture → encode |
| PR4B | Rust NativeEncodedSource | FFI bindings + shared Annex-B parse / H264 packetize / RTCP PLI impl |
| PR5 | Structured config direct path | `sources_v2` → `create_source_from_spec()` bypasses legacy URL for native sources |
| PR6 | Docs & config cleanup | Updated `conf/live777.toml`, this document |

### What was NOT removed

- Old `bridge_ffi.cpp` / `bridge_v4l2_ffi.cpp` / `bridge_v4l2_rdk_ffi.cpp` — kept for compatibility.
- `legacy_url.rs` and its `parse_libcamera_url()` / `parse_v4l2_url()` / `parse_rtp_url()` — still used by Path A configs.
- Legacy `libcamera://` and `v4l2://` URL support — fully functional.
