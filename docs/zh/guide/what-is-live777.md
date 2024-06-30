# Live777 是什么？

简单，高性能的流媒体服务器

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

