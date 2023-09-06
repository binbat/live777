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

## Tools

We have tools for support rtp -> whip/whep convert

For Example:

```bash
ffmpeg -> rtp2whip -> live777 -> whep2rtp -> ffplay
```

### rtp2whip

```bash
cargo run --package=rtp2whip -- -c vp8 -u http://localhost:3000/whip/777 --port 5003
```

Video Test Src

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'
```

So. We support parameter `command`, You can use this:

```bash
cargo run --package=rtp2whip -- -c vp8 -u http://localhost:3000/whip/777 --command \
"ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp 'rtp://127.0.0.1:{port}?pkt_size=1200'"
```

### whep2rtp

```bash
cargo run --package=whep2rtp -- -c vp8 -u http://localhost:3000/whep/777 -t localhost:5004
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

So. You can use this:

```bash
cargo run --package=whep2rtp -- -c vp8 -u http://localhost:3000/whep/777 -t 127.0.0.1:5004 --command 'ffplay -protocol_whitelist rtp,file,udp -i stream.sdp'
```

