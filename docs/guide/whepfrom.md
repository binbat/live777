# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

This tool has three working mode:
- `rtp`
- `rtsp as client`
- `rtsp as server`

## RTP

```bash
whepfrom -o output.sdp -w http://localhost:7777/whep/777
```

Use [`ffplay`](/guide/ffmpeg) play

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

Use [`vlc`](/guide/vlc) play

```bash
vlc output.sdp
```

## RTSP Server

It's default mode

This example is `whepfrom` as RTSP Server, use `ffplay` as client use RTSP pull stream

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

`whepfrom` as a client, push stream from RTSP Server

```bash
whepfrom -w http://localhost:7777/whip/777 -o rtsp://127.0.0.1:8554
```

### Use transport `tcp`

```bash
whepfrom -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/test-rtsp?transport=tcp
```

