# Live777

A very simple, high performance, edge WebRTC SFU (Selective Forwarding Unit)

## Demo

Current: Only supports rtp send, browser playing

### Browser

```shell
# open your browser
open http://localhost:3000/
```

### Use Cli WHIP / WHEP

```bash
docker build -t gstwebrtchttp .
docker run --network=host -it gstwebrtchttp gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! vp8enc error-resilient=partitions keyframe-max-dist=100 auto-alt-ref=true cpu-used=5 deadline=1 ! rtpvp8pay ! whipsink whip-endpoint="http://localhost:3000/whip/123"
```

### Send RTP to listening socket

You can use any software to send VP8 packets to port 5004.

#### GStreamer

```shell
gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! vp8enc error-resilient=partitions keyframe-max-dist=10 auto-alt-ref=true cpu-used=5 deadline=1 ! rtpvp8pay ! udpsink host=127.0.0.1 port=5004
```

#### ffmpeg

```shell
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp rtp://127.0.0.1:5004?pkt_size=1200
```
