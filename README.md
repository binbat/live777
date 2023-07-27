# Live777

A very simple, high performance, edge WebRTC SFU (Selective Forwarding Unit)

## Demo

Current: Only supports rtp send, browser playing

### Browser

```shell
# open your browser
open http://localhost:3000/
```

### Send RTP to listening socket

You can use any software to send VP8 packets to port 5004.

#### GStreamer
Analog Video Streaming
```shell
gst-launch-1.0 videotestsrc ! video/x-raw,width=640,height=480,format=I420 ! decodebin name=decoder ! queue ! video/x-raw ! videoconvert ! queue ! vp8enc deadline=1 ! rtpvp8pay ! queue ! whipsink name=ws use-link-headers=true auth-token="hellothere" whip-endpoint="ws://localhost:5004" decoder. ! queue ! audio/x-raw ! opusenc ! rtpopuspay ! queue ! ws.
```
Local video streaming
```shell
gst-launch-1.0 -e uridecodebin uri=file:///home/meh/Videos/spring-blender.mkv ! videoconvert ! whipwebrtcsink name=ws signaller::whip-endpoint="http://127.0.0.1:5004"
```

#### ffmpeg

```shell
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp rtp://127.0.0.1:5004?pkt_size=1200
```
