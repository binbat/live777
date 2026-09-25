# Live777 是什么？

简单，高性能的 `WHIP`/`WHEP` 协议优先的流媒体服务器

对于更大规模的部署，配套的 `liveman` 管理器可以将多个 Live777 节点组成集群：它将客户端请求代理到各个节点，管理节点之间的级联状态，并协调整个集群的录制。

Live777 可以将发布的流录制为分片 MP4（fMP4）片段，保存到本地文件系统或兼容 S3 的对象存储，并通过 REST API 进行控制。

Live777 内置嵌入式 WebUI、管理与会话 REST API 以及 Prometheus 指标，便于在生产环境中运维和观测。

面向 ARM 开发板的摄像头推流，内置的 **livehal** 原生管线在服务器进程内完成硬件采集与编码——不需要 ffmpeg 或其他辅助进程，也不占用 CPU 做软件编码，并自带自适应码率和运行时质量档位切换。目前支持的设备：

- **树莓派（Raspberry Pi）**——libcamera 采集 + V4L2 M2M 编码器（已在 Zero 2 W + OV5647 摄像头上验证）
- **瑞芯微 RK3588 / RV1126B**——V4L2 采集 + RKMPP 编码器，支持 DMA-BUF 零拷贝
- **地平线 RDK X5**——V4L2 采集 + RDK 硬件编码器
- **通用 Linux V4L2 开发板**——USB/CSI 摄像头 + V4L2 M2M 编码器

参见[树莓派部署指南](./raspberry-pi.md)和 [livehal](./livehal.md) 参考文档。

## 什么是 SFU Server ?

![webrtc-mesh-mcu-sfu](/webrtc-mesh-mcu-sfu.excalidraw.svg)

## 什么是 `WHIP`/`WHEP` 协议 ?

Live777 支持互联网上广泛使用的音视频协议转换，例如将 RTP 协议转换为 WHIP 或 WHEP 等其他协议。

![live777-arch](/live777-arch.excalidraw.svg)

Live777媒体服务器可与 [Gstreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) 等客户端配合使用，提供流媒体的接收与分发能力，采用典型的发布（推流）与订阅（播放）服务器模式。

