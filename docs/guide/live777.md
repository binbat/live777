# Live777 Core (liveion)

A Pure Single SFU Server for WebRTC.

`WHIP` / `WHEP` protocols are supported by default. When built with the `rtsp`
feature, liveion can also act as an RTSP server: push media in via
`ANNOUNCE/RECORD` and pull media out via `DESCRIBE/PLAY`.

a core SFU server, If you need a single server, use this

## Current support codecs {#codec}

| protocol | video codecs                        | audio codecs   |
| -------- | ----------------------------------- | -------------- |
| `WHIP`   | `AV1`, `VP9`, `VP8`, `H265`, `H264` | `Opus`, `G722` |
| `WHEP`   | `AV1`, `VP9`, `VP8`, `H265`, `H264` | `Opus`, `G722` |
| `RTSP`   | `AV1`, `VP9`, `VP8`, `H265`, `H264` | `Opus`, `G722` |

![live777-apps](/live777-apps.excalidraw.svg)

## Current client support {#client}

Application          | `AV1`  | `VP9`  | `VP8`  | `H265` | `H264` | `OPUS` | `G722` |
------------------   | :----: | :----: | :----: | :----: | :----: | :----: | :----: |
Browser `whip`       | :star: | :star: | :star: | :star: | :star: | :star: | :star: |
Browser `whep`       | :star: | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whip`     | :star: | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whep`     | :tv: 2 | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whipinto` | :tv: 1 | :star: | :star: | :star: | :star: | :tv: 1 | :star: |
Gstreamer `whepfrom` | :tv: 2 | :star: | :star: | :star: | :star: | :star: | :star: |
FFmpeg `whipinto`    | :shit: | :star: | :star: | :star: | :star: | :star: | :star: |
FFmpeg `whepfrom`    | :shit: | :star: | :star: | :star: | :star: | :star: | :star: |
VLC `whipinto`       | :shit: | :shit: | :star: | :star: | :star: | :star: | :shit: |
VLC `whepfrom`       | :shit: | :shit: | :star: | :star: | :star: | :star: | :shit: |
OBS Studio `whip`    | :tv: 3 | :shit: | :shit: | :star: | :star: | :star: | :shit: |

- :star: It's working
- :shit: Don't support
- :bulb: I don't know, No testing
- :tv: Note have some problem
  1. Working, But Browser can't player this video, Gstreamer to Gstreamer is working
  2. I don't know why av1 and whep error
  3. [OBS av1 codec can't play](https://github.com/binbat/live777/issues/169)

## Authentication

### No Authentication {#noauth}

::: danger
No Authentication is Default
:::

If no set any about `[auth]` section in configuration file, There will no authentication

### Bearer token {#token}

Static HTTP bearer token is super admin access, you should use in develop, test, debug or cluster manager

```toml
# WHIP/WHEP auth token
# Headers["Authorization"] = "Bearer {token}"
[auth]
# static JWT token, superadmin, debuggger can use this token
tokens = ["live777"]
```

### JWT(JSON Web Token) {#JWT}

Use this authentication, the token include access, you can control stream, publish, subscribe...

```toml
# WHIP/WHEP auth token
# Headers["Authorization"] = "Bearer {token}"
[auth]
# JSON WEB TOKEN secret
secret = "<jwt_secret>"
```

## Cascade

### What is cascade?

![cascade](/cascade.excalidraw.svg)

### Very large cluster

![mash-cascade](/mash-cascade.excalidraw.svg)

live777 Cascade have two mode:
- `cascade-pull`
- `cascade-push`

![live777-cascade](/live777-cascade.excalidraw.svg)

## DataChannel Forward

> NOTE: About `createDataChannel()`
> 1. Live777 Don't support `label`, `createDataChannel(label)` this `label` is not used
> 2. Live777 Don't support `negotiated`, `{ id: 42, negotiated: true }` this don't support

![live777-datachannel](/live777-datachannel.excalidraw.svg)

## RTSP Server

Build with the `rtsp` feature to expose an RTSP server alongside WHIP/WHEP:

```bash
cargo build --release --bin live777 --features rtsp
```

Configure the listen addresses in `live777.toml`:

```toml
[rtsp]
push_listen = "0.0.0.0:8554"   # ANNOUNCE/RECORD input
pull_listen = "0.0.0.0:8555"   # DESCRIBE/PLAY output
```

- Push media into a stream: `rtsp://host:8554/{stream_id}`
- Pull media from a stream: `rtsp://host:8555/{stream_id}`

Both UDP and TCP (`RTP/AVP/TCP`) transports are supported. The first URL path
segment is used as the liveion stream identifier. Authentication is not yet
implemented for the RTSP interface.

