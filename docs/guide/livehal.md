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
- **DMA-BUF zero-copy** (generic V4L2 → RKMPP): implemented.  With `prefer_dmabuf = true` on both capture and encoder, the capture exports each V4L2 buffer as a DMA-BUF (`VIDIOC_EXPBUF`) and the encoder imports it directly — no CPU copy of the frame.  Buffer ownership is explicit: the capture buffer is owned by the encoder between `VIDIOC_DQBUF` and the frame's encode completion, and is requeued only after the hardware has finished reading it (a deferred-requeue release contract, so the camera can never overwrite a buffer mid-encode).  If export or import fails the pipeline falls back to the CPU-copy path.  The RDK backend does not implement DMA-BUF import yet (`encoder_rdk.cpp` rejects `BufferKind::DmaBuf`); there everything is still copied through the CPU path.

## Config

Sources are configured under per-stream `[[stream.<name>.sources]]` blocks in `conf/live777.toml`.
The stream name is the key under `[stream]`; each stream can have an
optional DataChannel <-> UDP channel. Only **one** source per stream runs at
a time — the source registry is keyed by stream name, so configure a single
source per stream.

### Provisioned streams and on-demand sources

Every `[stream.<name>]` entry is *provisioned*: the stream is registered at
startup, always appears in the API and Dashboard (even while idle), is
exempt from the automatic teardown strategies (orphan reaper,
`auto_delete_whip` / `auto_delete_whep`), and cannot be created or deleted
through the admin API (`POST` / `DELETE /api/streams/<name>` return 409).

A provisioned stream is permanent, but its media plane still follows
publisher lifecycles:

- When a WHIP publisher leaves gracefully, the stream is **reset to
  standby**: its subscribers are disconnected (WHEP cannot renegotiate
  tracks, so viewers must re-subscribe) and a `stream-deleted` +
  `stream-created` hook pair with reason `reset` fires. Always-on sources
  are restarted; on-demand sources stay stopped until the next subscriber.
- A WHIP publisher cannot attach while the stream's configured source is
  running (and vice versa) — that would mix two publishers' tracks; the
  publish request fails with 409.

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

Subscribers that arrive while a source start is still in flight wait for it
(and for each other) instead of receiving a track-less answer; if the source
does not become ready within `on_demand_start_timeout_ms` the subscribe
request fails and the client can retry. Source start/stop emits
`PublishStarted` / `PublishStopped` with the session id `virtual-source`,
which also drives recording (`recorder.auto_streams`) and
`on_publish_started` / `on_publish_stopped` hooks.

For WHEP URL sources, the effective source readiness wait is at least
`35000ms`, even if `on_demand_start_timeout_ms` is lower. This lets a cold
upstream WHEP/on-demand source complete its HTTP setup timeout and deliver
the first media packet.

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
| `encoder.backend` | `"v4l2-m2m"`, `"rdk"`, `"rkmpp"` |

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
| `encoder-rkmpp` | Rockchip MPP hardware encoder (aarch64 only) |

### Platform presets

| Preset | Expands to |
|--------|-----------|
| `native-rpi` | `capture-libcamera, capture-v4l2, encoder-v4l2-m2m` |
| `native-generic-v4l2` | `capture-v4l2, encoder-v4l2-m2m` |
| `native-rdk` | `capture-v4l2, encoder-rdk` |
| `native-rkmpp` | `capture-v4l2, encoder-rkmpp` |

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

# Rockchip RKMPP (RK3588, RV1126B)
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rkmpp,webui
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

For cross builds, `just rpi-cross-build` uses the GHCR cross image with a
Raspberry Pi libcamera sysroot baked in. Set `RPI_SYSROOT` only when overriding
that with a sysroot copied from a device.

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

### Rockchip RKMPP (RK3588, RV1126B)

```bash
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rkmpp,webui
```

Requires the Rockchip MPP library (`librockchip_mpp`) and headers. Set `RKMPP_SYSROOT` to a sysroot containing them when cross-compiling. The rkmpp encoder accepts NV12 input only; pair it with `pixel_format = "nv12"` capture.

> **Note:** rkmpp profile configuration is stricter than the other backends. For H.264, `profile` must be a 6-character profile-level-id hex string (e.g. `"420028"` = Baseline Level 4.0, `"640028"` = High Level 4.0) and a separate `level` field is not allowed — the level is encoded in the hex. For H.265, use `profile = "main"` with an optional `level` (`"3.0"`–`"5.1"`, default `"4.0"`); only Main profile / Main tier are supported.

The Cross Images workflow publishes `ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest`, an aarch64 cross image with the MPP sysroot baked in at `/opt/rkmpp-sysroot` (built from `docker/Dockerfile.cross-aarch64-rkmpp`). Point cross at it via `CROSS_TARGET_AARCH64_UNKNOWN_LINUX_GNU_IMAGE`, or use `just rkmpp-pack-size`; `RKMPP_SYSROOT` overrides the baked sysroot with one pulled from a device.

### macOS (development / check only)

```bash
cargo check --no-default-features
cargo check --features native-rpi,webui
```

> **Note:** On macOS and Windows, native backend features are silently skipped by the build script; CMake is not invoked and no native symbols are linked. You can run `cargo check` with native features for linting, but the resulting binary cannot use native sources on those platforms.

### Environment variables

| Variable | Purpose |
|----------|---------|
| `RPI_SYSROOT` | Optional Raspberry Pi sysroot with `libcamera-dev`; overrides the sysroot baked into the RPi cross image. |
| `RDK_SYSROOT` | Path to the Horizon RDK X5 SDK sysroot. **Required** when building `encoder-rdk` / `native-rdk` for aarch64. |
| `RKMPP_SYSROOT` | Path to a sysroot containing the Rockchip MPP library and headers. Used when building `encoder-rkmpp` / `native-rkmpp` for aarch64. |
| `LIVEHAL_CXX_STDLIB` | Override the C++ standard library to link (`stdc++`, `c++`, etc.). Useful for cross-compilation toolchains. |
| `LIVEHAL_RDK_ALLOW_UNDEFINED` | Set to `1` to allow unresolved symbols in RDK shared libraries during cross-compilation with an incomplete sysroot. |

## Backend selection (build-time)

The CMake backend is inferred from the enabled capture/encoder features:

| Enabled feature(s) | Selected backend | CMake defines (ON) |
|-------------------|------------------|-------------------|
| `capture-libcamera` | `rpi` | `ENABLE_BACKEND_PI`, `ENABLE_CAPTURE_LIBCAMERA`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |
| `encoder-rdk` on aarch64 | `rdk-x5` | `ENABLE_BACKEND_RDK_X5`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RDK_X5` |
| `encoder-rkmpp` on aarch64 | `rkmpp` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RKMPP` |
| `capture-v4l2` / `encoder-v4l2-m2m` | `generic-v4l2` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |

When no `capture-*` feature is enabled, CMake is skipped entirely. Encoder-only features do **not** trigger a CMake build — the SourcePipeline requires a capture backend.

`capture-libcamera` is mutually exclusive with `encoder-rdk` and `encoder-rkmpp`. If enabled together, the encoder feature is ignored with a build warning and the `rpi` (libcamera) backend is selected.

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

### Rockchip RKMPP (RK3588 / RV1126B)

```toml
[stream.rk-cam]
[[stream.rk-cam.sources]]

[stream.rk-cam.sources.capture]
backend = "v4l2"
device = "/dev/video0"
width = 1280
height = 720
fps = 60
pixel_format = "nv12"
prefer_dmabuf = true      # DMA-BUF zero-copy (capture side)

[stream.rk-cam.sources.encoder]
backend = "rkmpp"
codec = "h264"
bitrate = 4_000_000
profile = "420028"
gop = 120
prefer_dmabuf = true      # DMA-BUF zero-copy (encoder side)

[stream.rk-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

`prefer_dmabuf` must be set on **both** sides for the zero-copy path; with
either side unset (or if the driver cannot export/import the buffer) the
pipeline falls back to the CPU-copy path.  Input must be NV12.

## Raspberry Pi notes

Verified on a Raspberry Pi Zero 2 W (Debian 13) with the OV5647 (v1) camera.

### Sensor frame-rate limits

`fps` in the capture config is a request: libcamera clamps it to what the
selected sensor mode allows.  Measured per-mode ceilings (OV5647; the
selected mode's limit is also printed by `rpicam-hello` at startup):

| Mode | Max fps |
|------|---------|
| 640x480 | ~62.5 |
| 1296x972 | ~46 |
| 1920x1080 | ~33 |
| 2592x1944 | ~15.6 |

- **60 fps is only reachable at 640x480** on this sensor; asking for more
  simply runs at the ceiling.  120 fps is not possible.
- Rough CPU cost on a Zero 2 W (libcamera → v4l2-m2m H.264): ~20% of one
  core at 640x480@30, ~40% at 640x480@60, ~65% at 1296x972@30.

## Low-latency streaming

Glass-to-glass latency is dominated by frame cadence, not encoder speed:
every stage (sensor readout, capture queue, encoder input, network, player
jitter buffer) can hold a frame for up to one frame period.  Doubling the
frame rate halves the per-stage worst case — 30→60 fps (33.3 → 16.7 ms) is
typically worth 30–50 ms end-to-end.

The trade-off is exposure: the per-frame exposure budget is `1/fps`
(16.7 ms at 60 fps, 8.3 ms at 120 fps).  In low light the picture gets
darker and noisier.  On Raspberry Pi the pipeline sets
`FrameDurationLimits` from `fps`, so the frame rate stays locked — the AE
can only shorten exposure and raise gain (a darker/noisier image, but
latency is preserved).

### Latency budget on a LAN (Raspberry Pi)

No glass-to-glass number is published for the Pi yet — measure yours with
the QR-code timer method (camera films a millisecond timer on a
high-refresh display; photograph the WHEP playback next to the timer).
The budget at 640x480@60 on a LAN looks like:

- **Sensor cadence + ISP + capture**: 1–2 frame periods (17–33 ms at
  60 fps).
- **v4l2-m2m encoder**: a few ms per frame; it keeps up with 60 fps at VGA
  with plenty of headroom on a Zero 2 W (~40% of one core).
- **LAN network**: a few ms with host-candidate ICE on the same segment.
- **Browser player**: typically the largest single share — Chromium jitter
  buffering, decode and render usually account for half or more of the
  total.

So after frame rate, the remaining lever is the *player* (decode/render
path and jitter buffer), not the server.

### 60 fps reference config (Raspberry Pi)

```toml
[stream.cam]
[[stream.cam.sources]]

[stream.cam.sources.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 60            # OV5647 ceiling is ~62.5 at VGA
pixel_format = "yuv420"

[stream.cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 2_000_000
profile = "baseline"
level = "3.1"
gop = 120           # 2 s at 60 fps; a shorter GOP joins/recovers faster

[stream.cam.sources.output]
payload_type = 96
clock_rate = 90000
```

- Confirm the real rate from the encoder stats line
  (`[V4l2M2mEncoder] stats: injected=…`): it advances at the actual capture
  fps.
- Keep players on the same LAN (host-candidate ICE,
  `ice_udp_addrs = ["auto"]`) to avoid relay detours.
- Browser WHEP clients should gather ICE candidates before POSTing
  (full-SDP offer); trickle-only clients currently fail to connect — see
  issue [#417](https://github.com/binbat/live777/issues/417).
