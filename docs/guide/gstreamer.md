# Gstreamer

Gstreamer `WHIP`/`WHEP` client

This `WHIP`/ `WHEP` plugins from [gst-plugins-rs](https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs/)

You can use this docker [images](https://github.com/binbat/live777/pkgs/container/live777-client) of Gstreamer

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


`WHIP`:

```bash
docker run --name live777-client-whip --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! av1enc usage-profile=realtime ! av1parse ! rtpav1pay ! whipsink whip-endpoint="http://localhost:7777/whip/777"
```


```bash
docker run --name live777-client-whip --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! av1enc usage-profile=realtime ! av1parse ! rtpav1pay ! udpsink host=127.0.0.1 port=5003
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



