# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

This tool has three working mode:
- `rtp`
- TODO: `rtsp as client`
- TODO: `rtsp as server`

## RTP

```bash
whepfrom -o output.sdp -w http://localhost:7777/whep/777
```

Use `ffplay` play

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```
