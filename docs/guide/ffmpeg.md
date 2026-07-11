# FFmpeg

We have tools [whipinto](/guide/whipinto) and [whepfrom](/guide/whepfrom) for support `rtp` <-> `whip`/`whep` convert

For Example:

```
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

::: warning `ffmpeg/whip`
ffmpeg >= 8.0, ffmpeg support whip protocol.
[ffmpeg/whip](https://ffmpeg.org/ffmpeg-formats.html#whip-1)

Need build set flag `--enable-muxer=whip`

Most pre-compile don't enable this

```
ffmpeg -> live777 -> whepfrom -> ffplay
```

Only support codec `h264` and `opus`
:::

You can use this docker images ffmpeg:

```bash
docker build -f docker/Dockerfile.ffmpeg -t ghcr.io/binbat/ffmpeg .
```

## H264

### X264 RTP

Video Test Src

```bash
# send RTP and Create SDP file
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libx264 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
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

### X264 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/ffmpeg:latest \
\
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libx264 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f whip http://localhost:7777/whip/777
```

## H265

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libx265 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 25 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## AV1

::: warning
- Please set `-strict experimental`
:::

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 -pix_fmt yuv420p \
-c:v libaom-av1 -cpu-used 8 -tile-columns 0 -tile-rows 0 -row-mt 1 \
-lag-in-frames 0 -g 30 -keyint_min 30 -b:v 0 -crf 30 -threads 4 \
-strict experimental \
-f rtp "rtp://127.0.0.1:5002" -sdp_file input.sdp
```

## VP8

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libvpx -pix_fmt yuv420p \
-g 60 -keyint_min 60 \
-deadline realtime -speed 4 \
-b:v 2000k -maxrate 2500k -bufsize 5000k \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## VP9

::: warning
Packetizing VP9 is experimental and its specification is still in draft state. Please set `-strict experimental` in order to enable it.
:::

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p \
-g 60 -keyint_min 60 \
-deadline realtime -speed 5 \
-row-mt 1 -tile-columns 2 -frame-parallel 1 \
-b:v 1800k -maxrate 2200k -bufsize 4400k \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

::: warning
VP9 support multi color space, Must add `-pix_fmt yuv420p` params.
:::

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## OPUS

### OPUS RTP

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec libopus \
-ar 48000 -ac 2 \
-b:a 48k -application voip \
-frame_duration 10 -vbr constrained \
-f rtp 'rtp://127.0.0.1:5004'
```

### OPUS WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/ffmpeg:latest \
\
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-ac 2 -ar 48000 -acodec libopus \
-f whip http://localhost:7777/whip/777
```

## G722

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec g722 -f rtp 'rtp://127.0.0.1:5004?pkt_size=1200'
```

## Both

### VP8+OPUS RTP

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=1280x720:rate=30 \
-acodec libopus -vn -f rtp rtp://127.0.0.1:5002 \
-vcodec libvpx -an -f rtp rtp://127.0.0.1:5004 -sdp_file input.sdp
```

### H264+G722 RTP

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=1280x720:rate=30 \
-acodec g722 -vn -f rtp rtp://127.0.0.1:5002 \
-vcodec libx264 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-an -f rtp rtp://127.0.0.1:5004 \
-sdp_file input.sdp
```

### H264+G722 WHIP

```bash
docker run --rm --network host \
ghcr.io/binbat/ffmpeg:latest \
\
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=1280x720:rate=30 \
-ac 2 -ar 48000 -acodec libopus \
-vcodec libx264 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f whip http://localhost:7777/whip/777
```

