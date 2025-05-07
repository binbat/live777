# FFmpeg

我们有 [whipinto](/guide/whipinto) 和 [whepfrom](/guide/whepfrom) 来把 `rtp` <-> `whip`/`whep` 进行转换

例如:

```
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

## Video: VP8

视频测试源

```bash
# send RTP and Create SDP file
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 \
-vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
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

## Video: VP9

::: warning
VP9 打包功能尚处于实验阶段，其规范尚处于草案状态。请设置 `-strict experiment` 选项以启用此功能。
:::

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 \
-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

::: warning
VP9 支持多种颜色空间，必须添加 `-pix_fmt yuv420p` 参数。
:::

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## Video: H264

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 \
-profile:v baseline -level 3.0 -pix_fmt yuv420p \
-g 30 -keyint_min 30 -b:v 1000k \
-minrate 1000k -maxrate 1000k -bufsize 1000k \
-preset ultrafast -tune zerolatency \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

## Video: AV1

::: danger
RTP 不支持的编解码器 av1
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

