# livehal

libcamera / V4L2 / RDK X5 原生采集与编码管线的架构和构建指南。

`livehal` 是 Hardware Abstraction Layer（硬件抽象层）crate，封装了 `native-pipeline` C++ pipeline。它向 `liveion` 暴露安全的 Rust API（`NativePipeline`），同时将所有 FFI 细节保持在 crate 内部。

## 架构

```
┌──────────────────────────────────────────────────────────┐
│ liveion（RTP / WHEP / 源管理器）                          │
│                                                          │
│  NativeSource（统一薄封装层）                             │
│         │                                                │
│         ▼                                                │
│  NativeEncodedSource                                     │
│    webrtc-rs H264Payloader / Packetizer                  │
│    → MediaPacket::RtpPacket(Arc<Packet>)                 │
│    → inject_rtp（无 marshal/unmarshal 往返）              │
│         │                                                │
│  RTCP PLI → request_keyframe()                           │
│         │                                                │
│─────────│─ 可选依赖边界 ──────────────────────────────────│
│         ▼                                                │
│ livehal（原生后端 crate）                                 │
│                                                          │
│  NativePipeline（安全 Rust 封装）                         │
│         │                                                │
│         ▼  crate-private FFI                             │
│  native_ffi.rs  →  source_pipeline_ffi.h                 │
│         │                                                │
│         ▼                                                │
│ C++（native-pipeline）                                    │
│                                                          │
│  SourcePipeline                                          │
│    ├─ CaptureBackend  →  RawFrame（C++ 内部）             │
│    └─ EncoderBackend  →  EncodedPacket                   │
│                              │                           │
│                              ▼  FFI 回调                  │
│  on_encoded_packet()  ← EncodedPacketFFI                 │
│         │                                                │
│         ▼  数据立即拷贝 → mpsc 通道                       │
│  EncodedPacket → liveion NativeEncodedSource             │
└──────────────────────────────────────────────────────────┘
```

- **RawFrame** 仅在 C++ 内部使用，不会跨越 FFI 边界。
- **EncodedPacket** 通过 `livehal` 内的纯 C 回调跨越 FFI；数据会被立即拷贝，并通过 mpsc 通道发送给 `liveion`。
- 所有 FFI 细节在 `livehal` 内部都是 crate-private 的；`liveion` 只能通过通道看到 `EncodedPacket`。
- **原生源的 RTP 路径**：`EncodedPacket` → webrtc-rs `H264Payloader` / `Packetizer` → `MediaPacket::RtpPacket(Arc<Packet>)` → `track.inject_rtp`。这避免了其他源所使用的 `Packet` → bytes → `Packet::unmarshal` 往返。
- `MediaPacket::Rtp { data }` 字节路径仍由 `rtp_listener` / `rtsp_source` / `sdp_source` 使用。
- **DMA-BUF 零拷贝尚未实现。** `prefer_dmabuf` 配置字段已存在于 schema 中，并已贯通到 C++ 层。RDK V4L2 采集在 `prefer_dmabuf=true` 时会导出 DMA-BUF fd，但 `encoder_rdk.cpp` 尚未实现 DMA-BUF fd 导入——它会拒绝 `BufferKind::DmaBuf`。默认值仍为 `false`。目前所有帧都通过 CPU 路径拷贝。完整的用户态零拷贝需要在 RDK 编码后端实现 DMA-BUF 导入，并处理好采集缓冲区生命周期。

## 配置

源配置在 `conf/live777.toml` 中按 stream 划分，写在 `[[stream.<name>.sources]]` 下。
`[stream]` 下的每个键就是 stream 名称；每个 stream 可以选配一个
DataChannel <-> UDP 通道。每个 stream 同时只能运行**一个**源——源注册表以流名为键，
因此请为每个流只配置一个源。

### 预注册流与按需源（on-demand）

每个 `[stream.<name>]` 条目都是"预注册"（provisioned）的：流在启动时即注册，
即使空闲也始终出现在 API 和 Dashboard 中，不受自动回收策略影响
（orphan reaper、`auto_delete_whip` / `auto_delete_whep`），
也不能通过 admin API 创建或删除（`POST` / `DELETE /api/streams/<name>` 返回 409）。

预注册流是永久的，但其媒体面仍跟随推流生命周期：

- WHIP 推流端正常离开时，流会被**重置为待机**：其订阅者全部断线
  （WHEP 无法重协商 track，观众需重新订阅），并触发一对 reason 为
  `reset` 的 `stream-deleted` + `stream-created` hook。常驻源会重启；
  on-demand 源保持停止直到下一个订阅者到来。
- 配置源运行期间 WHIP 推流端无法挂载（反之亦然）——否则两个推流方的
  track 会混入所有订阅者，该推流请求返回 409。

默认情况下，流的源在服务器启动时无条件启动。设置 `on_demand = true` 后，
源只在有人观看时运行——摄像头 / 编码器 / RTSP 拉流在第一个订阅者
（WHEP、cascade push 或 RTSP 拉流）到来时才启动，在最后一个订阅者离开后停止：

```toml
[stream.cam1]
on_demand = true
# 最后一个订阅者离开后停止源的宽限时间（毫秒，默认 10000）
on_demand_close_after_ms = 10000
# 第一个订阅者等待源就绪的最长时间，超时后订阅请求失败（毫秒，默认 10000）
on_demand_start_timeout_ms = 10000

[[stream.cam1.sources]]
url = "rtsp://192.168.1.100:554/stream"
```

在源启动进行期间到达的订阅者会等待启动完成（以及彼此），而不是拿到一个
不含 track 的应答；若源在 `on_demand_start_timeout_ms` 内未就绪，订阅请求
失败，客户端可重试。源的启停会以 `virtual-source` 会话 ID 发出
`PublishStarted` / `PublishStopped` 事件，同时驱动录制
（`recorder.auto_streams`）和 `on_publish_started` / `on_publish_stopped` 钩子。

on-demand 流在空闲时 Dashboard 显示 `standby` 徽标，源运行时显示
`on-demand`；其他预注册流显示 `config` 徽标。

### 基于 URL 的方式（非原生：RTSP / SDP / RTP）

```toml
[stream.rtsp-cam]
[[stream.rtsp-cam.sources]]
url = "rtsp://192.168.1.100:554/stream"
```

### 结构化原生配置（libcamera / V4L2 / RDK）

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
profile = "baseline"     # 也可以是 6 位十六进制 profile-level-id，如 "42001f"
level = "3.1"            # 当 profile 为名称时必填
gop = 60

[stream.pi-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

`pixel_format` 和 `codec` 值在启动时就会被校验（未知值会尽早报错）。`capture` + `encoder` 与 `url` 互斥，源类型由 `capture.backend` 推导（`device` 对 libcamera 是 camera ID，对 v4l2 是设备路径）。

`conf/live777.toml` 自带注释掉的 Pi / RDK 示例。复制它们到你自己的配置中即可启用摄像头源。

### 后端命名

| 层级 | 取值 |
|-------|-------|
| `capture.backend` | `"libcamera"`, `"v4l2"` |
| `encoder.backend` | `"v4l2-m2m"`, `"rdk"` |

### pixel_format 取值

| TOML 字符串 | RawPixelFormat | 数值 |
|---|---|---|
| `yuyv`, `yuyv422` | Yuyv422 | 0 |
| `nv12` | Nv12 | 1 |
| `yuv420`, `yuv420p` | Yuv420p | 2 |
| `mjpeg` | Mjpeg | 3 |
| `rgb888`, `rgb` | Rgb888 | 4 |

### codec 取值

| TOML 字符串 | VideoCodec | 数值 |
|---|---|---|
| `h264` | H264 | 100 |
| `h265`, `hevc` | H265 | 101 |
| `av1` | Av1 | 102 |
| `vp8` | Vp8 | 103 |
| `vp9` | Vp9 | 104 |

## 特性标志

特性分为采集后端、编码后端和便捷预设三类。所有后端特性都隐含 `native-source`，而 `native-source` 又会启用 `source`（自动启动）和 `dep:livehal`。

### 采集后端

| 特性 | 后端 |
|---------|------|
| `capture-libcamera` | libcamera（树莓派 CSI 摄像头） |
| `capture-v4l2` | V4L2 视频采集（USB 摄像头、通用 Linux） |

### 编码后端

| 特性 | 后端 |
|---------|------|
| `encoder-v4l2-m2m` | V4L2 Memory-to-Memory 硬件编码器 |
| `encoder-rdk` | 地平线 RDK X5 硬件编码器 |

### 平台预设

| 预设 | 展开为 |
|--------|-----------|
| `native-rpi` | `capture-libcamera, capture-v4l2, encoder-v4l2-m2m` |
| `native-generic-v4l2` | `capture-v4l2, encoder-v4l2-m2m` |
| `native-rdk` | `capture-v4l2, encoder-rdk` |

无需额外加 `--features source`——预设已经包含自动启动。

```bash
# 树莓派 CSI
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rpi,webui

# 通用 Linux V4L2
cargo build --bin live777 --release \
  --no-default-features --features native-generic-v4l2,webui

# RDK X5
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rdk,webui
```

## 构建

### 前置要求

- CMake ≥ 3.16
- C++17 编译器（gcc 或 clang）
- 按需准备平台 SDK（libcamera、RDK sysroot）

### 树莓派（libcamera）

```bash
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rpi,webui
```

需要带有 libcamera-dev 的 Pi sysroot。如果 sysroot 不在默认路径，请设置 `PI_SYSROOT`。

### 通用 Linux V4L2

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

需要带有 `hb_media_codec` 库的 RDK sysroot。必须设置 `RDK_SYSROOT` 指向 sysroot 路径；没有默认值。

> **注意：** DMA-BUF 零拷贝编码路径尚未实现。详见上文“架构”章节中的 DMA-BUF 说明。

### macOS（仅开发 / 检查）

```bash
cargo check --no-default-features
cargo check --features native-rpi,webui
```

> **注意：** 在 macOS 和 Windows 上，原生后端特性会被构建脚本静默跳过；不会调用 CMake，也不会链接原生符号。你可以用原生特性运行 `cargo check` 做静态检查，但生成的二进制无法在这些平台上使用原生源。

### 环境变量

| 变量 | 用途 |
|----------|---------|
| `PI_SYSROOT` | 包含 `libcamera-dev` 的树莓派 sysroot 路径。在构建 `capture-libcamera` / `native-rpi` 时使用。 |
| `RDK_SYSROOT` | 地平线 RDK X5 SDK sysroot 路径。在 aarch64 上构建 `encoder-rdk` / `native-rdk` 时**必须**设置。 |
| `LIVEHAL_CXX_STDLIB` | 覆盖要链接的 C++ 标准库（如 `stdc++`、`c++` 等），用于交叉编译工具链。 |
| `LIVEHAL_RDK_ALLOW_UNDEFINED` | 设为 `1` 可在 sysroot 不完整时允许 RDK 共享库存在未解析符号。 |

## 后端选择（构建时）

CMake 后端根据启用的采集/编码特性推断：

| 启用的特性 | 选定后端 | CMake 开启的宏 |
|-------------------|------------------|-------------------|
| `capture-libcamera` | `rpi` | `ENABLE_BACKEND_PI`, `ENABLE_CAPTURE_LIBCAMERA`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |
| aarch64 上的 `encoder-rdk` | `rdk-x5` | `ENABLE_BACKEND_RDK_X5`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RDK_X5` |
| `capture-v4l2` / `encoder-v4l2-m2m` | `generic-v4l2` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |

当没有启用任何 `capture-*` 特性时，CMake 会被完全跳过。仅启用编码特性**不会**触发 CMake 构建——SourcePipeline 需要一个采集后端。

`capture-libcamera` 和 `encoder-rdk` 互斥。如果同时启用，构建脚本会发出警告并忽略 `encoder-rdk`，选择 `rpi`（libcamera）后端。

## 配置示例

### 树莓派 CSI

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

### 树莓派 USB V4L2

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
