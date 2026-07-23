# livehal

Architecture and build guide for the libcamera / V4L2 / RDK X5 native capture-and-encode pipeline.

`livehal` is the Hardware Abstraction Layer crate that wraps the `native-pipeline` C++ pipeline. It exposes a safe Rust API (`NativePipeline`) to `liveion` while keeping all FFI details crate-private.

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
│ livehal (native backend crate)                           │
│                                                          │
│  NativePipeline  (safe Rust wrapper)                     │
│         │                                                │
│         ▼  crate-private FFI                             │
│  native_ffi.rs  →  source_pipeline_ffi.h                 │
│         │                                                │
│         ▼                                                │
│ C++ (native-pipeline)                                    │
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
- **EncodedPacket** crosses FFI via a pure-C callback inside `livehal`; data is copied immediately and sent through an mpsc channel to `liveion`.
- All FFI details are crate-private in `livehal`; `liveion` only sees `EncodedPacket` through the channel.
- **RTP path for native sources**: `EncodedPacket` → webrtc-rs `H264Payloader` / `Packetizer` → `MediaPacket::RtpPacket(Arc<Packet>)` → `track.inject_rtp`.  This avoids the `Packet` → bytes → `Packet::unmarshal` roundtrip that other sources use.
- `MediaPacket::Rtp { data }` bytes path is still used by `rtp_listener` / `rtsp_source` / `sdp_source`.
- **DMA-BUF zero-copy is not yet implemented.**  The `prefer_dmabuf` config field exists in the schema and is plumbed through to the C++ layer.  RDK V4L2 capture exports DMA-BUF fds when `prefer_dmabuf=true`, but `encoder_rdk.cpp` has not yet implemented DMA-BUF fd import — it rejects `BufferKind::DmaBuf`.  The default remains `false`.  Currently all frames are copied through the CPU path.  Full userspace zero-copy requires implementing DMA-BUF import in the RDK encoder backend and handling capture buffer lifetime.

## Config

Sources are configured under per-stream `[[stream.<name>.sources]]` blocks in `conf/live777.toml`.
The stream name is the key under `[stream]`; each stream can have one or more sources and an
optional DataChannel <-> UDP channel.

### Provisioned streams and on-demand sources

Every `[stream.<name>]` entry is *provisioned*: the stream is registered at
startup, always appears in the API and Dashboard (even while idle), is
exempt from the automatic teardown strategies (orphan reaper,
`auto_delete_whip` / `auto_delete_whep`), and cannot be created or deleted
through the admin API (`POST` / `DELETE /api/streams/<name>` return 409).

By default a stream's sources start unconditionally at server startup. Set
`on_demand = true` to run them only while someone is watching — the camera /
encoder / RTSP pull stays off until the first subscriber (WHEP, cascade push,
or RTSP pull) arrives, and stops again after the last one leaves:

```toml
[stream.cam1]
on_demand = true
# Grace period after the last subscriber leaves before sources stop (default 10000)
on_demand_close_after_ms = 10000
# How long the first subscriber waits for the source to become ready before
# its subscribe request fails (default 10000)
on_demand_start_timeout_ms = 10000

[[stream.cam1.sources]]
url = "rtsp://192.168.1.100:554/stream"
```

On-demand streams show a `standby` badge in the Dashboard while idle and
`on-demand` while their sources are running; other provisioned streams are
marked `config`.

### URL-based (non-native: RTSP / SDP / RTP)

```toml
[stream.rtsp-cam]
[[stream.rtsp-cam.sources]]
url = "rtsp://192.168.1.100:554/stream"
```

### Structured native (libcamera / V4L2 / RDK)

```toml
[stream.pi-cam]
[[stream.pi-cam.sources]]

[stream.pi-cam.sources.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 30
pixel_format = "yuv420"

[stream.pi-cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 1_000_000
profile = "baseline"     # or a 6-digit hex profile-level-id such as "42001f"
level = "3.1"            # required when profile is a profile name
gop = 60

[stream.pi-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

`pixel_format` and `codec` values are validated at startup (unknown values error early).  `capture` + `encoder` are mutually exclusive with `url`; the source type is derived from `capture.backend` (`device` is a camera ID for libcamera or a path for v4l2).

`conf/live777.toml` ships with commented-out Pi / RDK examples.  Copy them into your own config to enable a camera source.

### Backend naming

| Layer | Value |
|-------|-------|
| `capture.backend` | `"libcamera"`, `"v4l2"` |
| `encoder.backend` | `"v4l2-m2m"`, `"rdk"` |

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

Features are split into capture backends, encoder backends, and convenience
presets.  All backend features imply `native-source`, which in turn enables
`source` (autostart) and `dep:livehal`.

### Capture backends

| Feature | Backend |
|---------|---------|
| `capture-libcamera` | libcamera (Raspberry Pi CSI cameras) |
| `capture-v4l2` | V4L2 video capture (USB cameras, generic Linux) |

### Encoder backends

| Feature | Backend |
|---------|---------|
| `encoder-v4l2-m2m` | V4L2 Memory-to-Memory hardware encoder |
| `encoder-rdk` | Horizon RDK X5 hardware encoder |

### Platform presets

| Preset | Expands to |
|--------|-----------|
| `native-rpi` | `capture-libcamera, capture-v4l2, encoder-v4l2-m2m` |
| `native-generic-v4l2` | `capture-v4l2, encoder-v4l2-m2m` |
| `native-rdk` | `capture-v4l2, encoder-rdk` |

No additional `--features source` is needed — presets include autostart.

```bash
# Raspberry Pi CSI
 cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rpi,webui

# Generic Linux V4L2
cargo build --bin live777 --release \
  --no-default-features --features native-generic-v4l2,webui

# RDK X5
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rdk,webui
```

## Build

### Prerequisites

- CMake ≥ 3.16
- A C++17 compiler (gcc or clang)
- Platform SDK as needed (libcamera, RDK sysroot)

### Raspberry Pi (libcamera)

```bash
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rpi,webui
```

Requires the Pi sysroot with libcamera-dev. Set `PI_SYSROOT` if the sysroot is not at the default path.

### Generic Linux V4L2

```bash
cargo build --bin live777 --release \
  --no-default-features --features native-generic-v4l2,webui
```

### RDK X5

```bash
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rdk,webui
```

Requires the RDK sysroot with `hb_media_codec` libraries. `RDK_SYSROOT` must be set to the sysroot path; there is no default.

> **Note:** The DMA-BUF zero-copy encode path is not yet implemented.  See the DMA-BUF notes in the Architecture section above.

### macOS (development / check only)

```bash
cargo check --no-default-features
cargo check --features native-rpi,webui
```

> **Note:** On macOS and Windows, native backend features are silently skipped by the build script; CMake is not invoked and no native symbols are linked. You can run `cargo check` with native features for linting, but the resulting binary cannot use native sources on those platforms.

### Environment variables

| Variable | Purpose |
|----------|---------|
| `PI_SYSROOT` | Path to the Raspberry Pi sysroot containing `libcamera-dev`. Used when building `capture-libcamera` / `native-rpi`. |
| `RDK_SYSROOT` | Path to the Horizon RDK X5 SDK sysroot. **Required** when building `encoder-rdk` / `native-rdk` for aarch64. |
| `LIVEHAL_CXX_STDLIB` | Override the C++ standard library to link (`stdc++`, `c++`, etc.). Useful for cross-compilation toolchains. |
| `LIVEHAL_RDK_ALLOW_UNDEFINED` | Set to `1` to allow unresolved symbols in RDK shared libraries during cross-compilation with an incomplete sysroot. |

## Backend selection (build-time)

The CMake backend is inferred from the enabled capture/encoder features:

| Enabled feature(s) | Selected backend | CMake defines (ON) |
|-------------------|------------------|-------------------|
| `capture-libcamera` | `rpi` | `ENABLE_BACKEND_PI`, `ENABLE_CAPTURE_LIBCAMERA`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |
| `encoder-rdk` on aarch64 | `rdk-x5` | `ENABLE_BACKEND_RDK_X5`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RDK_X5` |
| `capture-v4l2` / `encoder-v4l2-m2m` | `generic-v4l2` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |

When no `capture-*` feature is enabled, CMake is skipped entirely. Encoder-only features do **not** trigger a CMake build — the SourcePipeline requires a capture backend.

`capture-libcamera` and `encoder-rdk` are mutually exclusive. If both are enabled, `encoder-rdk` is ignored with a build warning and the `rpi` (libcamera) backend is selected.

## Config examples

### Raspberry Pi CSI

```toml
[stream.pi-cam]
[[stream.pi-cam.sources]]

[stream.pi-cam.sources.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 30
pixel_format = "yuv420"

[stream.pi-cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 1_000_000
profile = "baseline"
level = "3.1"
gop = 60

[stream.pi-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

### Raspberry Pi USB V4L2

```toml
[stream.usb-cam]
[[stream.usb-cam.sources]]

[stream.usb-cam.sources.capture]
backend = "v4l2"
device = "/dev/video2"
width = 640
height = 480
fps = 30
pixel_format = "yuyv"

[stream.usb-cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.usb-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

### RDK X5

```toml
[stream.rdk-cam]
[[stream.rdk-cam.sources]]

[stream.rdk-cam.sources.capture]
backend = "v4l2"
device = "/dev/video0"
width = 640
height = 480
fps = 30
pixel_format = "yuyv"

[stream.rdk-cam.sources.encoder]
backend = "rdk"
codec = "h264"
bitrate = 1_000_000
profile = "42001f"
gop = 60

[stream.rdk-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

