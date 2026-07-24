# WhipInto

`RTP`/`RTSP` to `WHIP` tool

This tool has three working mode:
- `rtp`
- `rtsp as client`
- `rtsp as server`

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-i`, `--input` | `sdp://0.0.0.0:8554` | Input source: `sdp://` (RTP/SDP file or RTSP server mode), `rtsp://` (RTSP client mode), `synth://` (generated test frames) |
| `-w`, `--whip` | required | WHIP endpoint URL |
| `-t`, `--token` | none | Bearer token for WHIP authentication |
| `--command` | none | Run a command as child process |
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]` (empty string disables ICE servers) |
| `-v` | `warn` | Increase verbosity (`-v` info, `-vv` debug, `-vvv` trace) |

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

It's default mode

This example is `whipinto` as RTSP Server, use `ffmpeg` as client use RTSP push stream

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

`whipinto` as a client, pull stream from RTSP Server

```bash
whipinto -i rtsp://127.0.0.1:8554 -w http://localhost:7777/whip/777
```

### Use transport `tcp`

```bash
whipinto -i rtsp://localhost:8554/test-rtsp?transport=tcp -w http://localhost:7777/whip/test-rtsp
```

## Synthetic input

With the `rsmpeg` feature enabled, `-i` also accepts a `synth://` URL that
generates test frames in-process (no external encoder needed):

```bash
whipinto -i 'synth://h264?audio=opus&width=1280&height=720&fps=30' \
  -w http://localhost:7777/whip/777
```

Format: `synth://<vcodec>?<parameters>`; `<vcodec>` is one of `vp8`, `vp9`,
`h264`, `h265`, `av1` and every parameter is optional:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `audio` | none | Audio codec: `opus`, `g722` (omit for no audio) |
| `width` | `640` | Video width in pixels |
| `height` | `480` | Video height in pixels |
| `fps` | `30` | Video frame rate |
| `duration` | none | Stop publishing after this many seconds |
| `ice` | `--ice-server` value | ICE server spec `<url>[,<username>[,<credential>]]`, repeatable; replaces the CLI list for this input (an empty value disables ICE servers) |

## About `pkt_size=1200`

::: warning
WebRTC must need `pkt_size<=1200`

If `pkt_size > 1200` (most tool default `> 1200`, for example: `ffmpeg` default `1472`), we need to de-payload after re-payload
:::

But now, We support re-size `pkt_size` in `AV1`, `VP8` and `VP9`, You can use any `pkt_size` value in `AV1`, `VP8` and `VP9`

Codec             | `AV1`  | `VP9`  | `VP8`  | `H264` | `H265` | `OPUS` | `G722` |
----------------- | ------ | ------ | ------ | ------ | ------ | ------ | ------ |
`pkt_size > 1200` | :star: | :star: | :star: | :star: | :star: | :star: | :shit: |

- :star: It's working
- :shit: Don't support

