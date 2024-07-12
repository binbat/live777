# FFmpeg

We have tools for support rtp -> whip/whep convert

For Example:

```bash
ffmpeg -> whipinto -> live777 -> whepfrom -> ffplay
```

### whipinto

**NOTE: About `pkt_size=1200`**

WebRTC must need `pkt_size=1200`

If `pkt_size > 1200` (most tool `> 1200`, for example: `ffmpeg` default `1472`), we need to de-payload after re-payload

But now, We support re-size `pkt_size` in `VP8` and `VP9`, You can use any `pkt_size` value in `VP8` and `VP9`

Codec             | `AV1`  | `VP9`  | `VP8`  | `H264` | `OPUS` | `G722` |
----------------- | ------ | ------ | ------ | ------ | ------ | ------ |
`pkt_size > 1200` | :shit: | :star: | :star: | :shit: | :shit: | :shit: |

* * *

This tool is `rtp2whip`

Build

```bash
cargo build --package=whipinto --release
```

```bash
whipinto -c vp8 -u http://localhost:7777/whip/777 --port 5003
```

Video Test Src

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'
```

So. We support parameter `command`, You can use this:

```bash
cargo run --package=whipinto -- -c vp8 -u http://localhost:7777/whip/777 --command \
"ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp 'rtp://127.0.0.1:{port}?pkt_size=1200'"
```

### whepfrom

This tool is `whep2rtp`

Build

```bash
cargo build --package=whepfrom --release
```

Use WHEP protocol pull stream convert rtp sender

```bash
whepfrom -c vp8 -u http://localhost:7777/whep/777 -t localhost:5004
```

For ffplay. You Need a sdp file

```bash
cat > stream.sdp << EOF
v=0
m=video 5004 RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
EOF
```

Use ffplay

```bash
ffplay -protocol_whitelist rtp,file,udp -i stream.sdp
```

So. You can use this

```bash
cargo run --package=whepfrom -- -c vp8 -u http://localhost:7777/whep/777 -t 127.0.0.1:5004 --command 'ffplay -protocol_whitelist rtp,file,udp -i stream.sdp'
```

