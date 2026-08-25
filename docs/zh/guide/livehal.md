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
- **DMA-BUF 零拷贝**（通用 V4L2 → RKMPP）：已实现。采集和编码同时设 `prefer_dmabuf = true` 时，采集侧把每个 V4L2 缓冲区导出为 DMA-BUF（`VIDIOC_EXPBUF`），编码器直接导入使用——整帧不再经过 CPU 拷贝。缓冲区所有权是显式的：从 `VIDIOC_DQBUF` 到该帧编码完成期间缓冲区归编码器所有，待硬件读取完毕后才 requeue 回 V4L2（延迟 requeue 释放契约，摄像头不可能在编码中途覆盖缓冲区）。导出或导入失败时回退 CPU 拷贝路径。RDK 后端尚未实现 DMA-BUF 导入（`encoder_rdk.cpp` 会拒绝 `BufferKind::DmaBuf`），该路径仍以 CPU 拷贝为默认。

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

WHEP URL 源的有效源就绪等待时间至少为 `35000ms`，即使
`on_demand_start_timeout_ms` 配得更低也是如此。这样冷启动的上游
WHEP/on-demand 源可以完成 HTTP setup 超时预算并送出第一包媒体。

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

### 自适应码率(实验性)

在 encoder 段设置 `adaptive_bitrate = true` 后,运行中的编码器目标码率由
WHEP 订阅端的 RTCP 反馈驱动(issue #409):每秒采样一次丢包(优先 TWCC,
否则用 Receiver Report 的 `fraction_lost`),由 AIMD 控制器在运行时调整编码器。

```toml
[stream.pi-cam.sources.encoder]
bitrate = 4_000_000        # 上限 —— 控制器只从这里往下调
adaptive_bitrate = true
min_bitrate = 500_000      # 下限(默认:max(bitrate / 8, 300_000))
```

语义:丢包连续两个窗口超过 5% 就把码率降 15%;持续 10 秒无丢包则回升目标
码率的 5%。多个订阅者时按最差链路决定 —— 共享编码器会被一起拉低,单个弱网
订阅者会拖累整条流(根本解法是 simulcast)。加入不到 5 秒的订阅者不纳入
统计(解码器启动期的丢包属正常)。运行时调码率目前仅 `rkmpp` 后端支持;其他
后端会打印警告并自动停用控制器。

### 后端命名

| 层级 | 取值 |
|-------|-------|
| `capture.backend` | `"libcamera"`, `"v4l2"` |
| `encoder.backend` | `"v4l2-m2m"`, `"rdk"`, `"rkmpp"` |

### pixel_format 取值

| TOML 字符串 | RawPixelFormat | 数值 |
|---|---|---|
| `yuyv`, `yuyv422` | Yuyv422 | 0 |
| `nv12` | Nv12 | 1 |
| `yuv420`, `yuv420p` | Yuv420p | 2 |
| `mjpeg` | Mjpeg | 3 |
| `rgb888`, `rgb` | Rgb888 | 4 |
| `uyvy`, `uyvy422` | Uyvy422 | 5 |

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
| `encoder-rkmpp` | 瑞芯微 MPP 硬件编码器（仅 aarch64） |

### 平台预设

| 预设 | 展开为 |
|--------|-----------|
| `native-rpi` | `capture-libcamera, capture-v4l2, encoder-v4l2-m2m` |
| `native-generic-v4l2` | `capture-v4l2, encoder-v4l2-m2m` |
| `native-rdk` | `capture-v4l2, encoder-rdk` |
| `native-rkmpp` | `capture-v4l2, encoder-rkmpp` |

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

# 瑞芯微 RKMPP（RK3588、RV1126B）
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rkmpp,webui
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

交叉编译时，`just rpi-cross-build` 默认使用已内置 Raspberry Pi libcamera
sysroot 的 GHCR cross 镜像。只有需要覆盖为从设备同步出的 sysroot 时才设置
`RPI_SYSROOT`。

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

### 瑞芯微 RKMPP（RK3588、RV1126B）

```bash
cargo build --bin live777 --release \
  --target aarch64-unknown-linux-gnu \
  --no-default-features --features native-rkmpp,webui
```

需要 Rockchip MPP 库（`librockchip_mpp`）及头文件。交叉编译时设置 `RKMPP_SYSROOT` 指向包含它们的 sysroot。rkmpp 编码器仅接受 NV12 输入，请搭配 `pixel_format = "nv12"` 采集。

> **注意：** rkmpp 的 profile 配置比其他后端更严格。H.264 时 `profile` 必须是 6 位 profile-level-id 十六进制串（如 `"420028"` = Baseline Level 4.0，`"640028"` = High Level 4.0），且不允许单独的 `level` 字段——level 已编码在十六进制串中。H.265 时使用 `profile = "main"`，可选 `level`（`"3.0"`–`"5.1"`，默认 `"4.0"`）；仅支持 Main profile / Main tier。

Cross Images 工作流会发布 `ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest`，这是一个内置 MPP sysroot（位于 `/opt/rkmpp-sysroot`，由 `docker/Dockerfile.cross-aarch64-rkmpp` 构建）的 aarch64 交叉编译镜像。通过 `CROSS_TARGET_AARCH64_UNKNOWN_LINUX_GNU_IMAGE` 指定给 cross 使用，或直接运行 `just rkmpp-pack-size`；设置 `RKMPP_SYSROOT` 可改用从设备拉取的 sysroot。

### macOS（仅开发 / 检查）

```bash
cargo check --no-default-features
cargo check --features native-rpi,webui
```

> **注意：** 在 macOS 和 Windows 上，原生后端特性会被构建脚本静默跳过；不会调用 CMake，也不会链接原生符号。你可以用原生特性运行 `cargo check` 做静态检查，但生成的二进制无法在这些平台上使用原生源。

### 环境变量

| 变量 | 用途 |
|----------|---------|
| `RPI_SYSROOT` | 可选的树莓派 sysroot，需包含 `libcamera-dev`；会覆盖 RPi cross 镜像内置的 sysroot。 |
| `RDK_SYSROOT` | 地平线 RDK X5 SDK sysroot 路径。在 aarch64 上构建 `encoder-rdk` / `native-rdk` 时**必须**设置。 |
| `RKMPP_SYSROOT` | 包含 Rockchip MPP 库和头文件的 sysroot 路径。在 aarch64 上构建 `encoder-rkmpp` / `native-rkmpp` 时使用。 |
| `LIVEHAL_CXX_STDLIB` | 覆盖要链接的 C++ 标准库（如 `stdc++`、`c++` 等），用于交叉编译工具链。 |
| `LIVEHAL_RDK_ALLOW_UNDEFINED` | 设为 `1` 可在 sysroot 不完整时允许 RDK 共享库存在未解析符号。 |

## 后端选择（构建时）

CMake 后端根据启用的采集/编码特性推断：

| 启用的特性 | 选定后端 | CMake 开启的宏 |
|-------------------|------------------|-------------------|
| `capture-libcamera` | `rpi` | `ENABLE_BACKEND_PI`, `ENABLE_CAPTURE_LIBCAMERA`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |
| aarch64 上的 `encoder-rdk` | `rdk-x5` | `ENABLE_BACKEND_RDK_X5`, `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RDK_X5` |
| aarch64 上的 `encoder-rkmpp` | `rkmpp` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_RKMPP` |
| `capture-v4l2` / `encoder-v4l2-m2m` | `generic-v4l2` | `ENABLE_CAPTURE_V4L2`, `ENABLE_ENCODER_V4L2_M2M` |

当没有启用任何 `capture-*` 特性时，CMake 会被完全跳过。仅启用编码特性**不会**触发 CMake 构建——SourcePipeline 需要一个采集后端。

`capture-libcamera` 与 `encoder-rdk`、`encoder-rkmpp` 互斥。如果同时启用，构建脚本会发出警告并忽略对应的编码特性，选择 `rpi`（libcamera）后端。

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

使用打包 UYVY 格式的采集设备时，可以设置 `pixel_format = "uyvy"`
（`uyvy422` 是其别名）。generic V4L2 路径会保留采集 stride，并将 UYVY
原样传递给 V4L2 M2M，因此编码器设备也必须声明支持 UYVY 输入。

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

### 瑞芯微 RKMPP（RK3588 / RV1126B）

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
prefer_dmabuf = true      # DMA-BUF 零拷贝（采集侧）

[stream.rk-cam.sources.encoder]
backend = "rkmpp"
codec = "h264"
bitrate = 4_000_000
profile = "420028"
gop = 120
prefer_dmabuf = true      # DMA-BUF 零拷贝（编码侧）

[stream.rk-cam.sources.output]
payload_type = 96
clock_rate = 90000
```

零拷贝路径需要**两侧都**设 `prefer_dmabuf`；任一侧未设置（或驱动无法导出/导入缓冲区）都会回退 CPU 拷贝路径。输入必须是 NV12。

## 树莓派说明

已在 Raspberry Pi Zero 2 W（Debian 13）+ OV5647（v1 摄像头）上实测验证。

### Sensor 帧率上限

采集配置里的 `fps` 是请求值：libcamera 会将其钳制到所选 sensor 模式允许的范围。实测各模式上限（OV5647；模式被选中时 `rpicam-hello` 启动日志也会打印该上限）：

| 模式 | 最高 fps |
|------|---------|
| 640x480 | ~62.5 |
| 1296x972 | ~46 |
| 1920x1080 | ~33 |
| 2592x1944 | ~15.6 |

- **60fps 只有 640x480 能达到**；请求更高只会按上限运行。120fps 不可能。
- Zero 2 W 上的大致 CPU 开销（libcamera → v4l2-m2m H.264）：640x480@30 约单核 20%，640x480@60 约 40%，1296x972@30 约 65%。

## 低延时推流

glass-to-glass 延时的主导因素是帧节拍而不是编码器速度：每一级（sensor 读出、采集队列、编码器输入、网络、播放器 jitter buffer）最多可以攒住一帧。帧率翻倍，每级的最坏等待减半——30→60fps（33.3 → 16.7ms）端到端通常能省 30–50ms。

代价是曝光：单帧曝光预算是 `1/fps`（60fps 最多 16.7ms，120fps 最多 8.3ms）。暗光下画面会更暗、噪点更多。树莓派上 pipeline 按 `fps` 设置 `FrameDurationLimits`，帧率保持锁定——AE 只能缩短曝光、提高增益（画面变暗/变噪，但延时保住）。

### 局域网延时预算（树莓派）

目前还没公布树莓派的 glass-to-glass 实测值——可用二维码计时法自行测量（摄像头拍摄高刷屏上的毫秒计时器，拍照对比 WHEP 播放画面与计时器读数）。640x480@60 局域网下的预算构成：

- **sensor 节拍 + ISP + 采集**：1~2 个帧周期（60fps 时 17~33ms)。
- **v4l2-m2m 编码器**：每帧几 ms;VGA 60fps 在 Zero 2 W 上余量充足（约单核 40%)。
- **局域网网络**：同网段 host 候选 ICE，几 ms。
- **浏览器播放器**：通常占比最大——Chromium 的 jitter buffer、解码和渲染一般占去一半以上。

所以在帧率之后，剩下的主要优化空间在**播放器侧**（解码/渲染路径和 jitter buffer)，而不是服务端。

### 60fps 参考配置（树莓派）

```toml
[stream.cam]
[[stream.cam.sources]]

[stream.cam.sources.capture]
backend = "libcamera"
device = "0"
width = 640
height = 480
fps = 60            # OV5647 VGA 模式上限约 62.5
pixel_format = "yuv420"

[stream.cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 2_000_000
profile = "baseline"
level = "3.1"
gop = 120           # 60fps 下 2 秒;GOP 越短,加入/恢复越快

[stream.cam.sources.output]
payload_type = 96
clock_rate = 90000
```

- 用编码器统计行（`[V4l2M2mEncoder] stats: injected=…`）确认真实帧率：它按实际采集 fps 增长。
- 播放器尽量与服务端同网段（host 候选 ICE，`ice_udp_addrs = ["auto"]`），避免中继绕行。
- 浏览器 WHEP 客户端应在 POST 前完成 ICE 候选收集（完整 SDP offer）；纯 trickle 的客户端目前连不上——见 issue [#417](https://github.com/binbat/live777/issues/417)。
