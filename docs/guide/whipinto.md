---
description: >-
  Bridge RTP, RTSP, or synthetic test media into a Live777 WHIP endpoint,
  with authentication and troubleshooting examples.
---

# WhipInto

`RTP`/`RTSP` to `WHIP` tool

::: warning Streaming a camera from a Raspberry Pi / ARM board?
Do not pair `whipinto` with a software-encoding ffmpeg on these boards —
the CPU cannot keep up. The `native-rpi` / `native-rkmpp` `live777` builds
capture and encode in hardware, in-process; see the
[Raspberry Pi deployment guide](/guide/raspberry-pi). `whipinto` is for
sources that are already encoded (IP cameras, RTP/RTSP feeds, files,
synthetic test sources).
:::

This tool has two working modes:
- `rtp`
- `rtsp as client`

Use [whepfrom](/guide/whepfrom) for the reverse WHEP → RTP/RTSP direction, or
follow the [three-terminal loop](/guide/getting-started#verify-the-rtp-bridge-in-three-terminals).

## Get it and prerequisites

Download a package or prebuilt binary using the
[installation guide](/guide/installation), or build from this repository with
`cargo build --release --bin whipinto`. You need a reachable Live777 WHIP
endpoint and an already-encoded RTP/RTSP source. FFmpeg is required only for
the examples below; `synth://` input requires a build with `--features=rsmpeg`.

## Bearer token

If `live777.toml` contains `tokens = ["live777"]` under `[auth]`, pass the
same token with `-t`:

```bash
whipinto -i input.sdp -t live777 -w http://127.0.0.1:7777/whip/777
```

Do not put tokens in URLs or commit them to shell history and configuration
files intended for publication.

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-i`, `--input` | `input.sdp` | Input source: SDP file path (RTP), `rtsp://` (RTSP client mode), `synth://` (generated test frames) |
| `-w`, `--whip` | required | WHIP endpoint URL |
| `-t`, `--token` | none | Bearer token for WHIP authentication |
| `--command` | none | Run a command as child process |
| `--cable-url` | none | Cable UDP bridge URL for DataChannel &lt;-&gt; UDP forwarding, e.g. `udp://0.0.0.0:9001?host=127.0.0.1&port=9000` |
| `--cable-label` | `-` | DataChannel label; joins the matching cable on the server |
| `--ice-server` | none | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]`. Defaults to none — host candidates only (WHIP/WHEP endpoints advertise their own ICE servers via Link headers) |
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

## Troubleshooting

Run with `-vv` to include WebRTC and bridge diagnostics.

| Symptom | What to check |
|---------|---------------|
| `401 Unauthorized` | Pass `-t <token>` and make sure it exactly matches the server's `[auth] tokens`; do not add a second `Bearer` prefix. |
| `403 Forbidden` | Authentication reached the server but a proxy or cluster policy rejected it; check Live777/liveman authorization and routing rules. |
| `404 Not Found` | Check the `/whip/<stream>` URL, server port and reverse-proxy path. Confirm you are publishing to Live777 rather than a WHEP endpoint. |
| `409 Conflict` | Another publisher or configured source owns the stream. Stop it, use another stream, or review `strategy.override_publisher`; cascade-pull publishers cannot be displaced. |
| Connected, but no media | Confirm the sender is emitting to the SDP ports, the SDP codecs match the encoded input, and RTSP/SDP inputs are reachable. Check packet size and codec support above. |
| ICE timeout or disconnected | Host candidates alone work only where the client and server can directly reach each other. For remote/NAT use, configure server ICE addresses and STUN/TURN, then pass `--ice-server` when the client needs it; see [live777 configuration](/guide/live777). |

