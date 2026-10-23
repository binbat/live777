# Gstreamer

Gstreamer `WHIP`/`WHEP` client

```
gstreamer::whipsink -> live777 -> gstreamer::whepsrc
```

We have tools [whipinto](/guide/whipinto) and [whepfrom](/guide/whepfrom) for support `rtp` <-> `whip`/`whep` convert

```
gstreamer -> whipinto -> live777 -> whepfrom -> gstreamer
```

This `WHIP`/ `WHEP` (`whipsink` and `whepsrc`) plugins and RTP AV1 (`rtpav1pay` and `rtpav1depay`) from [gst-plugins-rs](https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/)

```bash
apt install -y --no-install-recommends libglib2.0-dev libssl-dev \
    gstreamer1.0-tools gstreamer1.0-libav \
    libgstreamer1.0-dev libgstrtspserver-1.0-dev \
    libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
    libpango1.0-dev libgstreamer-plugins-bad1.0-dev gstreamer1.0-nice

apt install -y --no-install-recommends cargo cargo-c
# debian:trixie use gstreamer 1.26.2
wget https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/-/archive/gstreamer-1.26.2/gst-plugins-rs-gstreamer-1.26.2.tar.gz gst-plugins-rs-gstreamer.tar.gz

tar -xf gst-plugins-rs-gstreamer.tar.gz --strip-components 1

# whip / whep: protocol support
# gst-plugin-webrtchttp
cargo cinstall -p gst-plugin-webrtchttp --libdir=pkg/usr/lib/$(gcc -dumpmachine)

# rtpav1pay / rtpav1depay: RTP (de)payloader for the AV1 video codec.
cargo cinstall -p gst-plugin-rtp --libdir=pkg/usr/lib/$(gcc -dumpmachine)
```

You can use this docker [images](https://github.com/binbat/live777/pkgs/container/live777-client) of Gstreamer

```bash
docker build -f docker/Dockerfile.gstreamer -t ghcr.io/binbat/gstreamer .
```

## H264

### X264 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
x264enc tune=zerolatency speed-preset=ultrafast key-int-max=60 byte-stream=true ! \
h264parse ! rtph264pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### X264 RTP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
x264enc tune=zerolatency speed-preset=ultrafast key-int-max=60 byte-stream=true ! \
h264parse ! rtph264pay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=H264 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 H264/90000
EOF
```

```bash
cargo run --bin=whipinto -- -i i.sdp -w http://localhost/whip/777
```

### X264 WHEP

::: danger `TODO:`
- `whepsrc` and `live777` has some bug

[live777#340](https://github.com/binbat/live777/issues/340)
:::


```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" \
audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" \
video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! \
rtph264depay ! decodebin ! videoconvert ! fakesink
```

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtph264depay ! decodebin ! videoconvert ! aasink
```

Use `libav`

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264 media=video,clock-rate=90000" ! rtph264depay ! avdec_h264 ! videoconvert ! aasink
```

## H265

### X265 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
x265enc tune=zerolatency speed-preset=ultrafast key-int-max=60 qp=23 ! \
h265parse ! rtph265pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### X265 RTP

```bash
docker run --name gstreamer --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
x265enc tune=zerolatency speed-preset=ultrafast key-int-max=60 qp=23 ! \
h265parse config-interval=1 ! rtph265pay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=H265 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 H265/90000
EOF
```

```bash
cargo run --bin=whipinto -- -i i.sdp -w http://localhost/whip/777
```

## AV1

### AV1 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
av1enc usage-profile=realtime keyframe-max-dist=60 ! \
av1parse ! rtpav1pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### AV1 RTP

::: danger `TODO:`
- Can't player in web player
- ffplay can player this

[live777#341](https://github.com/binbat/live777/issues/341)
:::

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
av1enc usage-profile=realtime keyframe-max-dist=60 ! \
av1parse ! rtpav1pay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=AV1 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 AV1/90000
EOF
```

### AV1 WHEP

::: danger `TODO:`
- `whepsrc` and `live777` has some bug
- I don't know why av1 and whep error
:::

But, you can:

```bash
cargo run --bin=whepfrom -- -w http://localhost:7777/whep/777 -o rtp://127.0.0.1:5004
```

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 udpsrc port=5004 caps="application/x-rtp, media=(string)video, encoding-name=(string)AV1" ! rtpjitterbuffer ! rtpav1depay ! av1parse ! av1dec ! videoconvert ! aasink
```

```bash
gst-launch-1.0 videotestsrc ! av1enc usage-profile=realtime ! av1parse ! rtpav1pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

## VP8

### VP8 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
vp8enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60 ! \
rtpvp8pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### VP8 RTP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
vp8enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60 ! \
rtpvp8pay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=VP8 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 VP8/90000
EOF
```


### VP8 WHEP

::: danger `TODO:`
- `whepsrc` and `live777` has some bug
:::

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" \
audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" \
video-caps="application/x-rtp,payload=96,encoding-name=VP8,media=video,clock-rate=90000" \
! rtpvp8depay ! vp8dec ! videoconvert ! aasink
```

## VP9

### VP9 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
vp9enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60 row-mt=1 ! \
vp9parse ! rtpvp9pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### VP9 RTP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
vp9enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60 row-mt=1 ! \
vp9parse ! rtpvp9pay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=VP9 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 VP9/90000
EOF
```

### VP9 WHEP

::: danger `TODO:`
- `whepsrc` and `live777` has some bug
:::

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=98,encoding-name=VP9,media=video,clock-rate=90000" ! rtpvp9depay ! vp9dec ! videoconvert ! aasink
```

## OPUS

### OPUS WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 audiotestsrc is-live=true ! \
opusenc ! \
opusparse ! rtpopuspay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### OPUS RTP

::: danger `TODO:`
- There can't player in webui

[live777#342](https://github.com/binbat/live777/issues/342)
:::

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 audiotestsrc is-live=true ! \
opusenc ! \
opusparse ! rtpopuspay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=OPUS Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=audio 5002 RTP/AVP 96
a=rtpmap:96 OPUS/48000/2
EOF
```

### OPUS WHEP

::: danger `TODO:`
- `whepsrc` and `live777` has some bug
:::

```bash
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtpopusdepay ! opusdec ! audioconvert ! autoaudiosink
```

Maybe you can't play audio, we can audio to video display for ascii

```bash
gst-launch-1.0 whepsrc whep-endpoint="http://localhost:7777/whep/777" audio-caps="application/x-rtp,payload=111,encoding-name=OPUS,media=audio,clock-rate=48000" video-caps="application/x-rtp,payload=102,encoding-name=H264,media=video,clock-rate=90000" ! rtpopusdepay ! opusdec ! audioconvert ! wavescope ! videoconvert ! aasink
```

## G722

**GStreamer G722 need `avenc_g722` in `gstreamer-libav`**

### G722 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 audiotestsrc is-live=true ! \
avenc_g722 ! \
rtpg722pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```

### G722 RTP

```bash
docker run --name gstreamer --rm --network host \
ghcr.io/binbat/gstreamer:latest \
gst-launch-1.0 audiotestsrc is-live=true ! \
avenc_g722 ! \
rtpg722pay ! udpsink host=127.0.0.1 port=5002
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=G722 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=audio 5002 RTP/AVP 96
a=rtpmap:96 G722/8000/1
EOF
```

## Both

### H264+OPUS RTP

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
x264enc tune=zerolatency speed-preset=ultrafast key-int-max=60 byte-stream=true ! \
h264parse ! rtph264pay pt=96 ! udpsink host=127.0.0.1 port=5002 \
audiotestsrc is-live=true ! \
avenc_g722 ! rtpg722pay pt=97 ! udpsink host=127.0.0.1 port=5004
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=H264 + G722 Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 H264/90000
a=fmtp:96 packetization-mode=1
m=audio 5004 RTP/AVP 97
a=rtpmap:97 G722/8000
EOF
```

```bash
cargo run --bin=whipinto -- -i i.sdp -w http://localhost/whip/777
```

### VP8+OPUS RTP

::: danger `TODO:`
only audio in webui can't player
:::

```bash
docker run --rm --network host \
ghcr.io/binbat/gstreamer:latest \
\
gst-launch-1.0 videotestsrc is-live=true ! \
video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
vp8enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60 ! \
rtpvp8pay pt=96 ! udpsink host=127.0.0.1 port=5002 \
audiotestsrc is-live=true ! opusenc ! opusparse ! \
rtpopuspay pt=97 ! udpsink host=127.0.0.1 port=5004
```

```bash
cat > i.sdp << EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=VP8 + OPUS Test Stream
c=IN IP4 127.0.0.1
t=0 0
m=video 5002 RTP/AVP 96
a=rtpmap:96 VP8/90000
m=audio 5004 RTP/AVP 97
a=rtpmap:97 OPUS/48000/2
EOF
```

```bash
cargo run --bin=whipinto -- -i i.sdp -w http://localhost/whip/777
```

