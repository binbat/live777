<h1 align="center">
  <img src="./docs/logo.svg" alt="Live777" width="200">
  <br>Live777<br>
</h1>

[![Rust](https://github.com/binbat/live777/actions/workflows/rust.yml/badge.svg)](https://github.com/binbat/live777/actions/workflows/rust.yml)

[![GitHub release](https://img.shields.io/github/tag/binbat/live777.svg?label=release)](https://github.com/binbat/live777/releases)

Live777 is an SFU server for real-time video streaming based on the WHIP/WHEP protocol.

Live777 media server is used with [Gstreamer](https://gstreamer.freedesktop.org/), [FFmpeg](https://ffmpeg.org/), [OBS Studio](https://obsproject.com/), [VLC](https://www.videolan.org/), [WebRTC](https://webrtc.org/) and other clients to provide the ability to receive and distribute streams, and is a typical publishing (pushing) and subscription (playing) server model.

Live777 supports the conversion of audio and video protocols widely used in the Internet, such as RTP to WHIP or WHEP and other protocols. 

![live777-arch](./docs/live777-arch.excalidraw.svg#gh-light-mode-only)

![live777-arch](./docs/live777-arch.dark.svg#gh-dark-mode-only)

## Features

Live777 has the following characteristics:

- 📚 **Support WHIP/WHEP**

  The WHIP/WHEP protocol is implemented to improve interoperability with other WebRTC application modules without the need for custom adaptations.

- 🗃️ **SFU architecture** 

  Only responsible for forwarding, do not do confluence, transcoding and other resource overhead of the media processing work, the encoding and decoding work are respectively placed on the sender and the receiver.

- 🔍 **Multiple audio and video encoding formats support** 

  Support a variety of video encoding and audio encoding formats,providing a wider range of compatibility to help enable adaptive streaming.
  ## Current
  | protocol | video codecs                | audio codecs   |
  | -------- | --------------------------- | -------------- |
  | `WHIP`   | `AV1`, `VP8`, `VP9`, `H264` | `Opus`, `G722` |
  | `WHEP`   | `AV1`, `VP8`, `VP9`, `H264` | `Opus`, `G722` |

- 🌐 **Multiple platform support** 
  With rich multi-platform native support.

## Quickstart
### Run Live777 using docker:
```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```
### Publish stream

- Use docker of [Gstreamer](https://gstreamer.freedesktop.org/download/) to publish:
> *Note:supports multiple encoding formats* 

:one: **Video: AV1**
**Note:AV1 has a lot of problem** 

-   av1
    -   ✅ browser whip av1
    -   ✅ browser whep av1
    -   🚧 gstreamer whip av1
    -   ✅ gstreamer whep av1
    -   ✅ gstreamer rtp av1 src
    -   🚧 gstreamer rtp av1 sink
    -   ✅ ffmpeg rtp av1 src
    -   ✅ ffmpeg rtp av1 sink

`WHIP`:

```bash
docker run --name live777-client-whip --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! av1enc usage-profile=realtime ! av1parse ! rtpav1pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
```

`WHEP`:

I don't know why av1 and whep error

But, you can:

```bash
cargo run --package=whepfrom -- -c av1 -u http://localhost:3000/whep/777 -t 127.0.0.1:5004
```

```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 udpsrc port=5004 caps="application/x-rtp, media=(string)video, encoding-name=(string)AV1" ! rtpjitterbuffer ! rtpav1depay ! av1parse ! av1dec ! videoconvert ! aasink
```
:two: **Video: VP8**
 `WHIP`:
 ```bash
docker run --name live777-client-whip --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! vp8enc ! rtpvp8pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"  
 ```
 `WHEP`:
 ```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:3000/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=96,encoding-name=VP8,media=video,clock-rate=90000" ! rtpvp8depay ! vp8dec ! videoconvert ! aasink
 ```
:three: ***Video:VP9***
 `WHIP`:
 ``` bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! vp9enc ! rtpvp9pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
 ```
 `WHEP`:
 ```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:3000/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=98,encoding-name=VP9,media=video,clock-rate=90000" ! rtpvp9depay ! vp9dec ! videoconvert ! aasink
 ```
:four: ***Video:H264***
 `WHIP`:
 ```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! x264enc ! rtph264pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
 ```
 `WHEP`:
 ```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:3000/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtph264depay ! decodebin ! videoconvert ! aasink
 ```
 Use `libav`
 ```bash
docker run --name live777-client-whep --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:3000/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264 media=video,clock-rate=90000" ! rtph264depay ! avdec_h264 ! videoconvert ! aasink
 ```
:five: ***Audio: Opus*** 
`WHIP`:
```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! opusenc ! rtpopuspay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
```
 `WHEP`:

 ```bash
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:3000/whep/777"   audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtpopusdepay ! opusdec ! audioconvert ! autoaudiosink
 ```

Maybe you can't play audio, we can audio to video display for ascii

```bash
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:3000/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtpopusdepay ! opusdec ! audioconvert ! wavescope ! videoconvert ! aasink
```
:six: ***Audio: G722***
**GStreamer G722 need `avenc_g722` in `gstreamer-libav`**

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! avenc_g722 ! rtpg722pay ! whipsink whip-endpoint="http://localhost:3000/whip/777
```
- Or publish stream by [Gstreamer](https://gstreamer.freedesktop.org/download/) or [OBS]((https://obsproject.com/forum/threads/obs-studio-30-beta.168984/)) :
> Note:  
> 1. OBS Studio version **30 or higher**        
> 2. OBS WHIP Current only support **H264** video codecs and **Opus** audio codecs
### Play stream
- open your browser,enter the URL:```http://localhost:3000/```

## Tools

We have tools for support rtp -> whip/whep convert

![live777-apps](./docs/live777-apps.excalidraw.svg#gh-light-mode-only)

![live777-apps](./docs/live777-apps.dark.svg#gh-dark-mode-only)


For Example:

```bash
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

### whipinto

This tool is `rtp2whip`

Build

```bash
cargo build --package=whipinto --release
```

```bash
whipinto -c vp8 -u http://localhost:3000/whip/777 --port 5003
```

Video Test Src

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'
```

So. We support parameter `command`, You can use this:

```bash
cargo run --package=whipinto -- -c vp8 -u http://localhost:3000/whip/777 --command \
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
whepfrom -c vp8 -u http://localhost:3000/whep/777 -t localhost:5004
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
cargo run --package=whepfrom -- -c vp8 -u http://localhost:3000/whep/777 -t 127.0.0.1:5004 --command 'ffplay -protocol_whitelist rtp,file,udp -i stream.sdp'
```

Use VLC player

```bash
vlc stream.sdp
```
## Sponsors

<p align="center">
  <img src="https://resources.jetbrains.com/storage/products/company/brand/logos/jb_beam.svg" alt="JetBrains Logo (Main) logo.">
</p>

