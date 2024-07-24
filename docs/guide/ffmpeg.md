# FFmpeg

We have tools for support rtp -> whip/whep convert

For Example:

```bash
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

## Video: VP8

Video Test Src

```bash
# send RTP and Create SDP file
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file stream.sdp
```

For ffplay. You Need a sdp file

```bash
ffplay -protocol_whitelist rtp,file,udp -i stream.sdp
```

You can use `whipinto` tools receiver RTP and convert to WebRTC

```
# Use SDP file as input
whipinto -i stream.sdp -w http://localhost:7777/whip/777
```

## Video: VP9

::: warning
Packetizing VP9 is experimental and its specification is still in draft state. Please set -strict experimental in order to enable it.
:::

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -strict experimental -vcodec libvpx-vp9 -f rtp 'rtp://127.0.0.1:5002' -sdp_file stream.sdp

whipinto -i stream.sdp -w http://localhost:7777/whip/777
```

## Video: H264

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 \
-x264-params "level-asymmetry-allowed=1:packetization-mode=1:profile-level-id=42001f" \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file stream.sdp

whipinto -i stream.sdp -w http://localhost:7777/whip/777
```

## Video: AV1

::: danger
RTP Unsupported codec av1
:::

## Audio: Opus

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 -acodec libopus -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'
```

## Audio: G722

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 -acodec g722 -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'
```

