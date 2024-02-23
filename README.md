<h1 align="center">
  <img src="./assets/logo.svg" alt="Live777" width="200">
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
    <a href="https://twitter.com/binbatlab"><img src="https://img.shields.io/badge/-Twitter-red?style=social&logo=twitter" height=25></a>
</div>

---

Live777 is an SFU server for real-time video streaming for the `WHIP`/`WHEP` as first protocol.

Live777 media server is used with [Gstreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) and other clients to provide the ability to receive and distribute streams, and is a typical publishing (pushing) and subscription (playing) server model.

Live777 supports the conversion of audio and video protocols widely used in the Internet, such as RTP to WHIP or WHEP and other protocols.

![live777-arch](./docs/live777-arch.excalidraw.svg#gh-light-mode-only)
![live777-arch](./docs/live777-arch.dark.svg#gh-dark-mode-only)

## Features

Live777 has the following characteristics:

- 📚 **Support `WHIP`/`WHEP`**

  The WHIP/WHEP protocol is implemented to improve interoperability with other WebRTC application modules without the need for custom adaptations.

- 🗃️ **SFU architecture**

  Only responsible for forwarding, do not do confluence, transcoding and other resource overhead of the media processing work, the encoding and decoding work are respectively placed on the sender and the receiver.

- 🌐 **Multiple platform support**

  With rich multi-platform native support.

- 🔍 **Multiple audio and video encoding formats support**

  Support a variety of video encoding and audio encoding formats, providing a wider range of compatibility to help enable adaptive streaming.

### DataChannel Forward

> NOTE: About `createDataChannel()`
> 1. Live777 Don't support `label`, `createDataChannel(label)` this `label` is not used
> 2. Live777 Don't support `negotiated`, `{ id: 42, negotiated: true }` this don't support

![live777-datachannel](./docs/live777-datachannel.excalidraw.svg)

## Current support encode
| protocol | video codecs                | audio codecs   |
| -------- | --------------------------- | -------------- |
| `WHIP`   | `AV1`, `VP9`, `VP8`, `H264` | `Opus`, `G722` |
| `WHEP`   | `AV1`, `VP9`, `VP8`, `H264` | `Opus`, `G722` |

## Quickstart

### Run Live777 using docker:

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

**Open your browser, enter the URL: http://localhost:7777/**

### Gstreamer `WHIP`/`WHEP` client

This `WHIP`/ `WHEP` plugins from [gst-plugins-rs](https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/)

You can use this docker [images](https://github.com/binbat/live777/pkgs/container/live777-client) of Gstreamer

#### Video: AV1

`WHIP`:

```bash
docker run --name live777-client-whip --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! av1enc usage-profile=realtime ! av1parse ! rtpav1pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

`WHEP`:

I don't know why av1 and whep error

But, you can:

```bash
cargo run --package=whepfrom -- -c av1 -u http://localhost:7777/whep/777 -t 127.0.0.1:5004
```

```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 udpsrc port=5004 caps="application/x-rtp, media=(string)video, encoding-name=(string)AV1" ! rtpjitterbuffer ! rtpav1depay ! av1parse ! av1dec ! videoconvert ! aasink
```

#### Video: VP8

`WHIP`:

```bash
docker run --name live777-client-whip --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! vp8enc ! rtpvp8pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

`WHEP`:

```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=96,encoding-name=VP8,media=video,clock-rate=90000" ! rtpvp8depay ! vp8dec ! videoconvert ! aasink
```

#### Video: VP9

`WHIP`:

``` bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! vp9enc ! rtpvp9pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

 `WHEP`:

```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=98,encoding-name=VP9,media=video,clock-rate=90000" ! rtpvp9depay ! vp9dec ! videoconvert ! aasink
```

#### Video: H264

`WHIP`:

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! x264enc ! rtph264pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

`WHEP`:

```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtph264depay ! decodebin ! videoconvert ! aasink
```

Use `libav`

```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264 media=video,clock-rate=90000" ! rtph264depay ! avdec_h264 ! videoconvert ! aasink
```

#### Audio: Opus

`WHIP`:

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! opusenc ! rtpopuspay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

`WHEP`:

```bash
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtpopusdepay ! opusdec ! audioconvert ! autoaudiosink
```

Maybe you can't play audio, we can audio to video display for ascii

```bash
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtpopusdepay ! opusdec ! audioconvert ! wavescope ! videoconvert ! aasink
```

#### Audio: G722

**GStreamer G722 need `avenc_g722` in `gstreamer-libav`**

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! avenc_g722 ! rtpg722pay ! whipsink whip-endpoint="http://localhost:7777/whip/777
```

### OBS Studio WHIP client

> Note:
> 1. OBS Studio version [**30 or higher**](https://obsproject.com/forum/threads/obs-studio-30-beta.168984/)
> 2. OBS WHIP Current only support **H264** video codecs and **Opus** audio codecs

![obs whip](./obs-whip.avif)

#### Play stream

- open your browser, enter the URL: [`http://localhost:7777/`](http://localhost:7777/)

## Tools

We have tools for support rtp -> whip/whep convert

![live777-apps](./docs/live777-apps.excalidraw.svg#gh-light-mode-only)
![live777-apps](./docs/live777-apps.dark.svg#gh-dark-mode-only)


For Example:

```bash
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

Application        | `AV1`  | `VP9`  | `VP8`  | `H264` | `OPUS` | `G722` |
------------------ | ------ | ------ | ------ | ------ | ------ | ------ |
Browser whip       | :star: | :star: | :star: | :star: | :star: | :star: |
Browser whep       | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer whip     | :tv: 1 | :star: | :star: | :star: | :star: | :star: |
Gstreamer whep     | :tv: 2 | :star: | :star: | :star: | :star: | :star: |
Gstreamer rtp-into | :tv: 1 | :star: | :star: | :star: | :star: | :star: |
Gstreamer rtp-from | :tv: 2 | :star: | :star: | :star: | :star: | :star: |
FFmpeg rtp-into    | :shit: | :star: | :star: | :star: | :star: | :star: |
FFmpeg rtp-from    | :shit: | :star: | :star: | :star: | :star: | :star: |
VLC rtp-into       | :bulb: | :bulb: | :star: | :star: | :star: | :bulb: |
VLC rtp-from       | :bulb: | :bulb: | :star: | :star: | :star: | :bulb: |
OBS Studio whip    | :tv: 3 | :shit: | :shit: | :star: | :star: | :shit: |

- :star: It's working
- :shit: Don't support
- :bulb: I don't know, No testing
- :tv: Note have some problem
  1. Working, But Browser can't player this video
  2. I don't know why av1 and whep error
  3. [AV1 is now available, But not released](https://github.com/obsproject/obs-studio/pull/9331)

### whipinto

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

