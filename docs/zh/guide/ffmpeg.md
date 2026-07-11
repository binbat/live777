# FFmpeg

我们有 [whipinto](/guide/whipinto) 和 [whepfrom](/guide/whepfrom) 来把 `rtp` <-> `whip`/`whep` 进行转换

例如:

```
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

::: warning `ffmpeg/whip`
ffmpeg >= 8.0 版本之后支持 `whip` 协议
[ffmpeg/whip](https://ffmpeg.org/ffmpeg-formats.html#whip-1)

需要在构建时打开 `--enable-muxer=whip`

大部分预编译二进制包都没有启用这个

```
ffmpeg -> live777 -> whepfrom -> ffplay
```

目前只支持 `h264` 和 `opus` 编码
:::

可以使用这个 ffmpeg 的 Docker 镜像:

```bash
docker build -f docker/Dockerfile.ffmpeg -t ghcr.io/binbat/ffmpeg .
```

## H264

### X264 RTP

视频测试源

```bash
# send RTP and Create SDP file
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libx264 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

对于 ffplay，你需要一个 sdp 文件

```bash
ffplay -protocol_whitelist rtp,file,udp -i input.sdp
```

你可以使用 `whipinto` 工具接收 RTP 并转换为 WebRTC

```bash
# Use SDP file as input
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

```bash
whepfrom -o output.sdp -w http://localhost:7777/whep/777
```

对于 ffplay，你需要一个 sdp 文件

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
- 需要设置 `-strict experimental`
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
-vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## VP9

::: warning
VP9 打包功能尚处于实验阶段，其规范尚处于草案状态。请设置 `-strict experiment` 选项以启用此功能。
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
VP9 支持多种颜色空间，必须添加 `-pix_fmt yuv420p` 参数。
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
-acodec libopus -f rtp 'rtp://127.0.0.1:5004'
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

