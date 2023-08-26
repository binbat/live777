# Live777

A very simple, high performance, support WHIP/WHEP edge WebRTC SFU (Selective Forwarding Unit)

## Current

|protocol|video codecs|audio codecs|
|--------|------------|------------|
|WHIP|VP8,VP9,H264|Opus|
|WHEP|VP8,VP9,H264|Opus|

### Live777 Server

```bash
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:main live777
```

### Browser Demo Page

```shell
# open your browser
open http://localhost:3000/
```

## Use OBS Studio WHIP

- OBS Studio >= 30

**OBS WHIP Current only support `H264` video codecs and `Opus` audio codecs**

![obs whip](./obs-whip.avif)

## Use GStreamer WHIP/WHEP

### VP8

```bash
docker run --name live777-client --rm --network host ghcr.io/binbat/live777-client:main gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! vp8enc ! rtpvp8pay ! whipsink whip-endpoint="http://localhost:3000/whip/endpoint/777"
```

### VP9

``` bash
docker run --name live777-client --rm --network host ghcr.io/binbat/live777-client:main gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! vp9enc ! rtpvp9pay ! whipsink whip-endpoint="http://localhost:3000/whip/endpoint/777"
```

### H264

```bash
docker run --name live777-client --rm --network host ghcr.io/binbat/live777-client:main gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! x264enc tune=zerolatency ! rtph264pay ! whipsink whip-endpoint="http://localhost:3000/whip/endpoint/777"
```

