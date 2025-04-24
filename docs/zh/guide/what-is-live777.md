# Live777 是什么？

简单，高性能的 `WHIP`/`WHEP` 协议优先的流媒体服务器

## 什么是 SFU Server ?

![webrtc-mesh-mcu-sfu](/webrtc-mesh-mcu-sfu.excalidraw.svg)

## 什么是 `WHIP`/`WHEP` 协议 ?

Live777 支持互联网上广泛使用的音视频协议转换，例如将 RTP 协议转换为 WHIP 或 WHEP 等其他协议。

![live777-arch](/live777-arch.excalidraw.svg)

Live777媒体服务器可与 [Gstreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) 等客户端配合使用，提供流媒体的接收与分发能力，采用典型的发布（推流）与订阅（播放）服务器模式。

