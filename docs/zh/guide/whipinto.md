# WhipInto

`RTP`/`RTSP` to `WHIP` tool

这个工具应该有三种模式：
- `rtp`
- `rtsp as client`
- `rtsp as server`

## 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-i`, `--input` | `sdp://0.0.0.0:8554` | 输入源：`sdp://`（RTP/SDP 文件或 RTSP server 模式）、`rtsp://`（RTSP client 模式）、`synth://`（生成测试帧） |
| `-w`, `--whip` | 必填 | WHIP 端点 URL |
| `-t`, `--token` | 无 | WHIP 认证使用的 Bearer token |
| `--command` | 无 | 以子进程方式运行命令 |
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE 收集使用的服务器，可重复指定；格式 `<url>[,<username>[,<credential>]]`（空字符串表示禁用 ICE 服务器） |
| `-v` | `warn` | 提高日志级别（`-v` info，`-vv` debug，`-vvv` trace） |

## RTP

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

::: tip
你需要先创建一个 SDP 文件

可以用 `ffmpeg -sdp_file` flag 来创建 SDP 文件
:::

### RTP Only video

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libx264 -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp
```

### RTP Only audio

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec libopus \
-ar 48000 -ac 2 \
-b:a 48k -application voip \
-frame_duration 10 -vbr constrained \
-f rtp 'rtp://127.0.0.1:5004' -sdp_file input.sdp
```

### RTP Audio and Video

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=1280x720:rate=30 \
-acodec libopus -vn -f rtp rtp://127.0.0.1:11111 \
-vcodec libvpx -an -f rtp rtp://127.0.0.1:11113 -sdp_file input.sdp
```

## RTSP Server

默认是这种模式

这个例子是用 `whipinto` 作为 RTSP Server，用 `ffmpeg` 作为 client 用 RTSP 推流

```bash
whipinto -w http://localhost:7777/whip/777
```

### Only video

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec libvpx -f rtsp 'rtsp://127.0.0.1:8554'
```

```bash
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
-vcodec -pix_fmt yuv420p \
-g 60 -keyint_min 60 -crf 23 \
-preset ultrafast -tune zerolatency \
-profile:v main -level 4.1 \
-f rtsp rtsp://127.0.0.1:8554
```

### Only audio

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec libopus -f rtsp 'rtsp://127.0.0.1:8554'
```

### Audio and Video

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=1280x720:rate=30 \
-acodec libopus -vcodec libvpx \
-f rtsp 'rtsp://127.0.0.1:8554'
```

### Use transport `tcp`

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=1280x720:rate=30 \
-acodec libopus -vcodec libvpx \
-rtsp_transport tcp \
-f rtsp 'rtsp://127.0.0.1:8554'
```

## RTSP Client

`whipinto` 作为一个客户端，从其他的 RTSP Server 来拉流

```bash
whipinto -i rtsp://127.0.0.1:8554 -w http://localhost:7777/whip/777
```

### Use transport `tcp`

```bash
whipinto -i rtsp://localhost:8554/test-rtsp?transport=tcp -w http://localhost:7777/whip/test-rtsp
```

## 合成输入（Synthetic input）

启用 `rsmpeg` feature 后，`-i` 也接受 `synth://` URL，在进程内生成测试帧（无需外部编码器）：

```bash
whipinto -i 'synth://h264?audio=opus&width=1280&height=720&fps=30' \
  -w http://localhost:7777/whip/777
```

格式：`synth://<vcodec>?<parameters>`；`<vcodec>` 为 `vp8`、`vp9`、`h264`、`h265`、`av1` 之一，所有参数均可选：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `audio` | 无 | 音频编码：`opus`、`g722`（省略表示无音频） |
| `width` | `640` | 视频宽度（像素） |
| `height` | `480` | 视频高度（像素） |
| `fps` | `30` | 视频帧率 |
| `duration` | 无 | 发布指定秒数后停止 |
| `ice` | `--ice-server` 的值 | ICE 服务器规格 `<url>[,<username>[,<credential>]]`，可重复指定；对该输入替换命令行列表（空值表示禁用 ICE 服务器） |

## About `pkt_size=1200`

::: warning
WebRTC必须满足 `pkt_size<=1200`

当 `pkt_size > 1200` 时（多数工具默认值 `> 1200`，例如： `ffmpeg` 默认 `1472`)，需要进行解封装后重新封装处理
:::

不过现在，我们已经在 `AV1`、`VP8` 和 `VP9` 编解码器中支持重新调整 `pkt_size`，您可以在 `AV1`、`VP8` 和 `VP9` 中使用任意大小的 `pkt_size` 值

Codec             | `AV1`  | `VP9`  | `VP8`  | `H264` | `H265` | `OPUS` | `G722` |
----------------- | ------ | ------ | ------ | ------ | ------ | ------ | ------ |
`pkt_size > 1200` | :star: | :star: | :star: | :star: | :star: | :star: | :shit: |

- :star: 正常运行
- :shit: 不支持

