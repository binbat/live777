<h1 align="center">
  <img src="./web/public/logo.svg" alt="Live777" width="200">
  <br>Live777<br>
</h1>

<div align="center">

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

  With rich multi-platform native support.

- 🔍 **Multiple audio and video encoding formats support**

  Support a variety of video encoding and audio encoding formats, providing a wider range of compatibility to help enable adaptive streaming.

### Cascade

![live777-cascade](./docs/live777-cascade.excalidraw.svg)

### DataChannel Forward

> NOTE: About `createDataChannel()`
> 1. Live777 Don't support `label`, `createDataChannel(label)` this `label` is not used
> 2. Live777 Don't support `negotiated`, `{ id: 42, negotiated: true }` this don't support

![live777-datachannel](./docs/live777-datachannel.excalidraw.svg)

#### Play stream

- open your browser, enter the URL: [`http://localhost:7777/`](http://localhost:7777/)

## Tools

We have tools for support rtp -> whip/whep convert

![live777-apps](./docs/live777-apps.excalidraw.svg)

For Example:

```bash
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

### whipinto

**NOTE: About `pkt_size=1200`**

WebRTC must need `pkt_size=1200`

If `pkt_size > 1200` (most tool `> 1200`, for example: `ffmpeg` default `1472`), we need to de-payload after re-payload

But now, We support re-size `pkt_size` in `VP8` and `VP9`, You can use any `pkt_size` value in `VP8` and `VP9`

Codec             | `AV1`  | `VP9`  | `VP8`  | `H264` | `OPUS` | `G722` |
----------------- | ------ | ------ | ------ | ------ | ------ | ------ |
`pkt_size > 1200` | :shit: | :star: | :star: | :shit: | :shit: | :shit: |

* * *

This tool is `rtp2whip`

Build

```bash
cargo build --package=whipinto --release
```

```bash
whipinto -c vp8 -u http://localhost:7777/whip/777 --port 5003
```

Video Test Src

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'
```

So. We support parameter `command`, You can use this:

```bash
cargo run --package=whipinto -- -c vp8 -u http://localhost:7777/whip/777 --command \
"ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp 'rtp://127.0.0.1:{port}?pkt_size=1200'"
```

VLC RTP stream

**Note: VLC can't support all video codec**

```bash
vlc -vvv <INPUT_FILE> --sout '#transcode{vcodec=h264}:rtp{dst=127.0.0.1,port=5003}'
```

### whepfrom

This tool is `whep2rtp`

Build

```bash
cargo build --package=whepfrom --release
```

Use WHEP protocol pull stream convert rtp sender

```bash
whepfrom -c vp8 -u http://localhost:7777/whep/777 -t localhost:5004
```

For ffplay. You Need a sdp file

```bash
cat > stream.sdp << EOF
v=0
m=video 5004 RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
EOF
```

Use ffplay

```bash
ffplay -protocol_whitelist rtp,file,udp -i stream.sdp
```

So. You can use this

```bash
cargo run --package=whepfrom -- -c vp8 -u http://localhost:7777/whep/777 -t 127.0.0.1:5004 --command 'ffplay -protocol_whitelist rtp,file,udp -i stream.sdp'
```

Use VLC player

```bash
vlc stream.sdp
```
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

