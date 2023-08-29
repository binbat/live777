# Live777

A very simple, high performance, support WHIP/WHEP edge WebRTC SFU (Selective Forwarding Unit)

## Current

|protocol|video codecs|audio codecs|
|--------|------------|------------|
|`WHIP`|`VP8`,`VP9`,`H264`|`Opus`,`G722`|
|`WHEP`|`VP8`,`VP9`,`H264`|`Opus`,`G722`|

### Live777 Server

```bash
docker run --name live777-server --rm --network host \
ghcr.io/binbat/live777-server:latest live777
```

### Browser Demo Page

```bash
# open your browser
open http://localhost:3000/
```

## Use OBS Studio WHIP

- OBS Studio >= 30

**OBS WHIP Current only support `H264` video codecs and `Opus` audio codecs**

![obs whip](./obs-whip.avif)

## Use GStreamer WHIP/WHEP

### Video: VP8

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! vp8enc ! rtpvp8pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
```

### Video: VP9

``` bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! vp9enc ! rtpvp9pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
```

### Video: H264

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 videotestsrc ! videoconvert ! x264enc ! rtph264pay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
```

### Audio: Opus

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! opusenc ! rtpopuspay ! whipsink whip-endpoint="http://localhost:3000/whip/777"
```

### Audio: G722

**GStreamer G722 need `avenc_g722` in `gstreamer-libav`**

```bash
docker run --name live777-client --rm --network host \
ghcr.io/binbat/live777-client:latest \
gst-launch-1.0 audiotestsrc ! audioconvert ! avenc_g722 ! rtpg722pay ! whipsink whip-endpoint="http://localhost:3000/whip/777
```

