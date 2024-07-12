# Gstreamer

Gstreamer `WHIP`/`WHEP` 插件



`WHIP` / `WHEP` (`whipsink` 和 `whepsrc`) 插件和 RTP AV1 (`rtpav1pay` and `rtpav1depay`) 在 [gst-plugins-rs](https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/)

但是大部分 Linux 发型版没有提供 `gst-plugins-rs` 的包，可以自己来编译

```bash
apt install -y --no-install-recommends libglib2.0-dev libssl-dev \
    libgstreamer1.0-dev gstreamer1.0-tools gstreamer1.0-libav \
    libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    libpango1.0-dev libgstreamer-plugins-bad1.0-dev gstreamer1.0-nice

apt install -y --no-install-recommends cargo cargo-c
wget https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/-/archive/gstreamer-1.22.8/gst-plugins-rs-gstreamer-1.22.8.tar.gz gst-plugins-rs-gstreamer.tar.gz

tar -xf gst-plugins-rs-gstreamer.tar.gz --strip-components 1

# whip / whep: protocol support
# gst-plugin-webrtchttp
cargo cinstall -p gst-plugin-webrtchttp --libdir=pkg/usr/lib/$(gcc -dumpmachine)

# rtpav1pay / rtpav1depay: RTP (de)payloader for the AV1 video codec.
cargo cinstall -p gst-plugin-rtp --libdir=pkg/usr/lib/$(gcc -dumpmachine)
```

也可以使用我们编译好的 Docker [images](https://github.com/binbat/live777/pkgs/container/live777-client)

## Video: VP8

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

## Video: VP9

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

## Video: H264

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

## Video: AV1

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

```bash
gst-launch-1.0 videotestsrc ! av1enc usage-profile=realtime ! av1parse ! rtpav1pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

## Audio: Opus

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

## Audio: G722

**GStreamer G722 need `avenc_g722` in `gstreamer-libav`**

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! avenc_g722 ! rtpg722pay ! whipsink whip-endpoint="http://localhost:7777/whip/777
```

