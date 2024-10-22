<h1 align="center">
  <img src="./web/public/logo.svg" alt="Live777" width="200">
  <br>Live777<br>
</h1>

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

</div>

<div align="center">
    <a href="https://discord.gg/mtSpDNwCAz"><img src="https://img.shields.io/badge/-Discord-424549?style=social&logo=discord" height=25></a>
    &nbsp;
    <a href="https://t.me/binbatlib"><img src="https://img.shields.io/badge/-Telegram-red?style=social&logo=telegram" height=25></a>
    &nbsp;
    <a href="https://twitter.com/binbatlab"><img src="https://img.shields.io/badge/-Twitter-red?style=social&logo=x" height=25></a>
</div>

---

This work is completed by Xidian University.

Live777 is an SFU server for real-time video streaming for the `WHIP`/`WHEP` as first protocol.

Live777 media server is used with [Gstreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) and other clients to provide the ability to receive and distribute streams, and is a typical publishing (pushing) and subscription (playing) server model.

Live777 supports the conversion of audio and video protocols widely used in the Internet, such as RTP to WHIP or WHEP and other protocols.

![live777-arch](./docs/public/live777-arch.excalidraw.svg)

## Features

Live777 has the following characteristics:

- 📚 **Support `WHIP`/`WHEP`**

  The WHIP/WHEP protocol is implemented to improve interoperability with other WebRTC application modules without the need for custom adaptations.

- 🗃️ **P2P/SFU Mix architecture**

  Only responsible for forwarding, do not do confluence, transcoding and other resource overhead of the media processing work, the encoding and decoding work are respectively placed on the sender and the receiver.

- 🌐 **Multiple platform support**

  Linux, MacOS, Windows, Android and arm, x86 with multi-platform native support.

### DataChannel Forward

> NOTE: About `createDataChannel()`
> 1. Live777 Don't support `label`, `createDataChannel(label)` this `label` is not used
> 2. Live777 Don't support `negotiated`, `{ id: 42, negotiated: true }` this don't support

![live777-datachannel](./docs/live777-datachannel.excalidraw.svg)

## Sponsors

<p align="center">
  <a href="https://faculty.xidian.edu.cn/NGC/zh_CN/index.htm">
    <img src="https://upload.wikimedia.org/wikipedia/en/2/2c/Xidian_logo.png" height="200" alt="Xi'an Electrical Science and Technology University logo.">
  </a>
  <br/>
  <a href="https://www.jetbrains.com/">
    <img src="https://resources.jetbrains.com/storage/products/company/brand/logos/jb_beam.svg" height="200" alt="JetBrains Logo (Main) logo.">
  </a>
  <br/>
  <a href="https://www.hostker.net/">
    <img src="https://kerstatic.cloud-open-api.com/email-img/hostker-logo.png" height="80" alt="Hostker logo.">
  </a>
</p>

