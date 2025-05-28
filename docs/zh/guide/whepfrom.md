# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

这个工具应该有三种模式，目前只实现了一种：
- `rtp`
- `rtsp as client`
- `rtsp as server`

## RTP

RTP 模式需要 `target` 和 `sdp file`

```bash
whepfrom -o rtp://{target_ip}?video={video_port}&audio={audio_port} -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

```bash
whepfrom -o rtp://localhost?video=9000&audio=9002 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

使用 [`ffplay`](/guide/ffmpeg) 来播放

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

使用 [`vlc`](/guide/vlc) 来播放

```bash
vlc output.sdp
```

## RTSP Server

默认是这种模式

这个例子是用 `whepfrom` 作为 RTSP Server，用 `ffplay` 作为 client 用 RTSP 拉流

```bash
whepfrom -w http://localhost:7777/whep/777 -o rtsp-listen://0.0.0.0:8551
```

### Player

```bash
ffplay rtsp://localhost:8551
```

### Use transport `tcp`

```bash
ffplay rtsp://localhost:8551 -rtsp_transport tcp
```

## RTSP Client

`whepfrom` 作为一个客户端，忘其他的 RTSP Server 来推流

```bash
whepfrom -w http://localhost:7777/whip/777 -o rtsp://127.0.0.1:8554
```

### Use transport `tcp`

```bash
whepfrom -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/test-rtsp?transport=tcp
```

