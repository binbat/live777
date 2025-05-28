# WhipInto

`RTP`/`RTSP` to `WHIP` tool

This tool has three working mode:
- `rtp`
- `rtsp as client`
- `rtsp as server`

## RTP

```bash
whipinto -i input.sdp -w http://localhost:7777/whip/777
```

::: tip
You need to generate an sdp file first

For example: Use `ffmpeg -sdp_file` flag
:::

### RTP Only video

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 \
-vcodec libvpx -f rtp 'rtp://127.0.0.1:5003' -sdp_file input.sdp
```

### RTP Only audio

```bash
ffmpeg -re -f lavfi -i sine=frequency=1000 \
-acodec libopus -f rtp 'rtp://127.0.0.1:5005' -sdp_file input.sdp
```

### RTP Audio and Video

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=640x480:rate=30 \
-acodec libopus -vn -f rtp rtp://127.0.0.1:11111 \
-vcodec libvpx -an -f rtp rtp://127.0.0.1:11113 -sdp_file input.sdp
```

## RTSP Server

It's default mode

This example is `whipinto` as RTSP Server, use `ffmpeg` as client use RTSP push stream

```bash
whipinto -w http://localhost:7777/whip/777
```

### Only video

```bash
ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 \
-vcodec libvpx -f rtsp 'rtsp://127.0.0.1:8554'
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
-f lavfi -i testsrc=size=640x480:rate=30 \
-acodec libopus -vcodec libvpx \
-f rtsp 'rtsp://127.0.0.1:8554'
```

### Use transport `tcp`

```bash
ffmpeg -re \
-f lavfi -i sine=frequency=1000 \
-f lavfi -i testsrc=size=640x480:rate=30 \
-acodec libopus -vcodec libvpx \
-rtsp_transport tcp \
-f rtsp 'rtsp://127.0.0.1:8554'
```

## RTSP Client

`whipinto` as a client, pull stream from RTSP Server

```bash
whipinto -i rtsp://127.0.0.1:8554 -w http://localhost:7777/whip/777
```

### Use transport `tcp`

```bash
whipinto -i rtsp://localhost:8554/test-rtsp?transport=tcp -w http://localhost:7777/whip/test-rtsp
```

## About `pkt_size=1200`

::: warning
WebRTC must need `pkt_size<=1200`

If `pkt_size > 1200` (most tool default `> 1200`, for example: `ffmpeg` default `1472`), we need to de-payload after re-payload
:::

But now, We support re-size `pkt_size` in `VP8` and `VP9`, You can use any `pkt_size` value in `VP8` and `VP9`

Codec             | `AV1`  | `VP9`  | `VP8`  | `H264` | `OPUS` | `G722` |
----------------- | ------ | ------ | ------ | ------ | ------ | ------ |
`pkt_size > 1200` | :shit: | :star: | :star: | :star: | :star: | :shit: |

- :star: It's working
- :shit: Don't support

