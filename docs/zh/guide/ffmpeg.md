# FFmpeg

我们有 [whipinto](/guide/whipinto) 和 [whepfrom](/guide/whepfrom) 来把 `rtp` <-> `whip`/`whep` 进行转换

For Example:

```
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

## Video: VP8

Video Test Src

```bash
# send RTP and Create SDP file
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 \
-vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

For ffplay. You Need a sdp file

```bash
ffplay -protocol_whitelist rtp,file,udp -i input.sdp
```

You can use `whipinto` tools receiver RTP and convert to WebRTC

```bash
# Use SDP file as input
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

```bash
whepfrom -o output.sdp -w http://localhost:7777/whep/777
```

For ffplay. You Need a sdp file

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

## Video: VP9

::: warning
Packetizing VP9 is experimental and its specification is still in draft state. Please set -strict experimental in order to enable it.
:::

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 \
-strict experimental -vcodec libvpx-vp9 \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## Video: H264

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 \
-x264-params "level-asymmetry-allowed=1:packetization-mode=1:profile-level-id=42001f" \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## Video: AV1

::: danger
RTP Unsupported codec av1
:::

## Audio: Opus

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec libopus -f rtp 'rtp://127.0.0.1:5004'
```

## Audio: G722

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec g722 -f rtp 'rtp://127.0.0.1:5004?pkt_size=1200'
```

## Both: Audio and Video

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=640x480:rate=30 \
-acodec libopus -vn -f rtp rtp://127.0.0.1:5002 \
-vcodec libvpx -an -f rtp rtp://127.0.0.1:5004 -sdp_file input.sdp
```

