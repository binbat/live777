# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

这个工具应该有三种模式，目前只实现了一种：
- `rtp`
- TODO: `rtsp as client`
- TODO: `rtsp as server`

## RTP

```bash
whepfrom -o output.sdp -w http://localhost:7777/whep/777
```

使用 [`ffplay`](/guide/ffmpeg) 来播放

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

使用 [`vlc`](/guide/vlc) 来播放

```bash
vlc output.sdp
```

