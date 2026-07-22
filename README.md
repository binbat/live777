<h1 align="center">
  <img src="./web/public/logo.svg" alt="Live777" width="200">
  <br>Live777<br>
</h1>

<p align="center">
  <b>A real-time audio and video streaming media server</b>
</p>

<div align="center">
  <a href="https://live777.pages.dev/guide/what-is-live777">
    <b>Documentation</b>
  </a>
  |
  <a href="https://live777.pages.dev/zh/guide/what-is-live777">
    <b>中文文档</b>
  </a>

  <br/>
  <br/>
</div>

<div align="center">

[![codecov](https://codecov.io/gh/binbat/live777/graph/badge.svg)](https://codecov.io/gh/binbat/live777)
[![Rust](https://github.com/binbat/live777/actions/workflows/rust.yml/badge.svg)](https://github.com/binbat/live777/actions/workflows/rust.yml)
[![GitHub release](https://img.shields.io/github/tag/binbat/live777.svg?label=release)](https://github.com/binbat/live777/releases)
[![License: MPL 2.0](https://img.shields.io/badge/License-MPL_2.0-brightgreen.svg)](./LICENSE)

</div>

<div align="center">
    <a href="https://discord.gg/mtSpDNwCAz"><img src="https://img.shields.io/badge/-Discord-424549?style=social&logo=discord" height=25></a>
    &nbsp;
    <a href="https://t.me/binbatlib"><img src="https://img.shields.io/badge/-Telegram-red?style=social&logo=telegram" height=25></a>
    &nbsp;
    <a href="https://twitter.com/binbatlab"><img src="https://img.shields.io/badge/-Twitter-red?style=social&logo=x" height=25></a>
</div>

---

**Live777** is a very simple, high performance, lightweight WebRTC SFU (**Selective Forwarding Unit**) server, with `WHIP`/`WHEP` as its first-class protocols.

Live777 works with [GStreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) and other clients to receive and distribute streams — a typical publish (push) / subscribe (play) server model. It also converts between the audio and video protocols widely used on the Internet, such as RTP/RTSP to `WHIP` and `WHEP`.

![live777-arch](./docs/public/live777-arch.excalidraw.svg)

## Features

- 📚 **Support `WHIP`/`WHEP`**

  Implements the WHIP/WHEP protocols for out-of-the-box interoperability with other WebRTC application modules — no custom adaptation required.

- 🗃️ **P2P/SFU Mix architecture**

  Only forwards media streams — no mixing, transcoding or other resource-hungry media processing on the server. Encoding and decoding stay on the sender and the receiver respectively.

- 🌐 **Multiple platform support**

  Native support for Linux, macOS, Windows and Android, on both ARM and x86.

- 🕸️ **Cluster & cascade**

  The companion `liveman` manager turns multiple Live777 nodes into a cluster — proxying client requests, managing cascade state between nodes, and coordinating recording across the cluster.

- 🎥 **Stream recording**

  Record published streams as fragmented MP4 segments to the local filesystem or S3-compatible object storage, controlled through the REST API.

- 📊 **WebUI & observability**

  An embedded WebUI, admin and session REST APIs, and Prometheus metrics are built in for easy operation and observability in production.

## Quick Start

Run Live777 with Docker (host networking is required):

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

Then open your browser at `http://localhost:7777/`.

For prebuilt binaries, Linux packages, winget and cargo installs, see the [installation guide](https://live777.pages.dev/guide/installation).

## Tools

This repository also ships these companion tools:

| Tool | Description |
| ---- | ----------- |
| `liveman` | Cluster manager for multiple Live777 nodes |
| `whipinto` | Push RTP/RTSP streams into a WHIP endpoint |
| `whepfrom` | Pull a WHEP stream and output RTP/RTSP |
| `net4mqtt` | TCP/UDP-over-MQTT proxy and tunnel |

## Sponsors

This work is supported by Xidian University.

<p align="center">
  <a href="https://faculty.xidian.edu.cn/NGC/zh_CN/index.htm">
    <img src="https://upload.wikimedia.org/wikipedia/en/2/2c/Xidian_logo.png" height="200" alt="Xidian University logo.">
  </a>
  <br/>
  <a href="https://www.jetbrains.com/">
    <img src="https://resources.jetbrains.com/storage/products/company/brand/logos/jb_beam.svg" height="200" alt="JetBrains logo.">
  </a>
  <br/>
  <a href="https://www.hostker.net/">
    <img src="https://kerstatic.cloud-open-api.com/email-img/hostker-logo.png" height="80" alt="Hostker logo.">
  </a>
</p>
