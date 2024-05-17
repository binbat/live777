# What is Live777

A very simple, high performance, edge WebRTC SFU

## Components

### live777

a core SFU server, If you need a single server, use this

### live777-gateway

live777 Cluster mode extra. need database

### whipinto and whepfrom

a protocol converter

- `RTP` => `WHIP`
- `WHEP` => `RTP`
- TODO: `RTSP` => `WHIP`
- TODO: `WHEP` => `RTSP`

