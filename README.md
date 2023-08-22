# Live777

A very simple, high performance, support WHIP/WHEP edge WebRTC SFU (Selective Forwarding Unit)

## Current

|protocol|video codecs|audio codecs|
|--------|------------|------------|
|WHIP|VP8|Opus|
|WHEP|VP8|Opus|

### Browser Demo Page

```shell
# open your browser
open http://localhost:3000/
```

### Use GStreamer WHIP/WHEP

```bash
docker build -t gstwebrtchttp .
docker run --network=host -it gstwebrtchttp gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! vp8enc ! rtpvp8pay ! whipsink whip-endpoint="http://localhost:3000/whip/endpoint/123"
```

