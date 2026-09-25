# What is Live777 ?

A very simple, high performance, edge WebRTC SFU(**Selective Forwarding Unit**) Server

Live777 is an SFU server for real-time video streaming for the `WHIP`/`WHEP` as first protocol.

For larger deployments, the companion `liveman` manager turns multiple Live777 nodes into a cluster: it proxies client requests to the nodes, manages cascade state between them, and coordinates recording across the cluster.

Live777 can record published streams as fragmented MP4 segments to the local filesystem or S3-compatible object storage, controlled through its REST API.

An embedded WebUI, admin and session REST APIs, and Prometheus metrics are built in, making the server easy to operate and observe in production.

For camera streaming on ARM boards, the built-in **livehal** native pipeline captures and hardware-encodes video inside the server process — no ffmpeg or other helper processes, and no CPU-heavy software encoding. Adaptive bitrate and runtime quality-tier switching are built in. Currently supported devices:

- **Raspberry Pi** — libcamera capture + V4L2 M2M encoder (verified on Zero 2 W with the OV5647 camera)
- **Rockchip RK3588 / RV1126B** — V4L2 capture + RKMPP encoder, with DMA-BUF zero-copy
- **Horizon RDK X5** — V4L2 capture + RDK hardware encoder
- **Generic Linux V4L2 boards** — USB/CSI cameras + V4L2 M2M encoder

See the [Raspberry Pi deployment guide](./raspberry-pi.md) and the [livehal](./livehal.md) reference.

## What is SFU Server ?

![webrtc-mesh-mcu-sfu](/webrtc-mesh-mcu-sfu.excalidraw.svg)

## What is `WHIP`/`WHEP` Protocol ?

Live777 supports the conversion of audio and video protocols widely used in the Internet, such as RTP to WHIP or WHEP and other protocols.

![live777-arch](/live777-arch.excalidraw.svg)

Live777 media server is used with [Gstreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) and other clients to provide the ability to receive and distribute streams, and is a typical publishing (pushing) and subscription (playing) server model.


