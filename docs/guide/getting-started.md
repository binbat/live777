# Getting Started

## Introduction

### Current support encode

| protocol | video codecs                | audio codecs   |
| -------- | --------------------------- | -------------- |
| `WHIP`   | `AV1`, `VP9`, `VP8`, `H264` | `Opus`, `G722` |
| `WHEP`   | `AV1`, `VP9`, `VP8`, `H264` | `Opus`, `G722` |

![live777-apps](/live777-apps.excalidraw.svg)

### Current client support

Application        | `AV1`  | `VP9`  | `VP8`  | `H264` | `OPUS` | `G722` |
------------------ | ------ | ------ | ------ | ------ | ------ | ------ |
Browser whip       | :star: | :star: | :star: | :star: | :star: | :star: |
Browser whep       | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer whip     | :tv: 1 | :star: | :star: | :star: | :star: | :star: |
Gstreamer whep     | :tv: 2 | :star: | :star: | :star: | :star: | :star: |
Gstreamer rtp-into | :tv: 1 | :star: | :star: | :star: | :star: | :star: |
Gstreamer rtp-from | :tv: 2 | :star: | :star: | :star: | :star: | :star: |
FFmpeg rtp-into    | :shit: | :star: | :star: | :star: | :star: | :star: |
FFmpeg rtp-from    | :shit: | :star: | :star: | :star: | :star: | :star: |
VLC rtp-into       | :shit: | :shit: | :star: | :star: | :star: | :shit: |
VLC rtp-from       | :shit: | :shit: | :star: | :star: | :star: | :shit: |
OBS Studio whip    | :tv: 3 | :shit: | :shit: | :star: | :star: | :shit: |

- :star: It's working
- :shit: Don't support
- :bulb: I don't know, No testing
- :tv: Note have some problem
  1. Working, But Browser can't player this video, Gstreamer to Gstreamer is working
  2. I don't know why av1 and whep error
  3. [OBS av1 codec can't play](https://github.com/binbat/live777/issues/169)

## Components

### Live777 Core (liveion)

A Pure Single SFU Server for WebRTC.

Only `WHIP` / `WHEP` protocol supported.

a core SFU server, If you need a single server, use this

### Live777 Manager (liveman)

Live777 Cluster manager.

If I have so many servers (live777 core cluster), I need this manage them all.

Cluster mode must liveman

![live777-cluster](/live777-cluster.excalidraw.svg)

### whipinto and whepfrom

Stream protocol convert tool.

- `RTP` => `WHIP`
- `WHEP` => `RTP`
- `RTSP` => `WHIP`
- TODO: `WHEP` => `RTSP`

### whipinto

This tool is `rtp2whip`

```bash
whipinto -w http://localhost:7777/whip/777
```

[Detail whipinto](whipinto)

### whepfrom

This tool is `whep2rtp`

Use WHEP protocol pull stream convert rtp sender

```bash
whepfrom -o <stream.sdp> -w http://localhost:7777/whep/777
```

[Detail whepfrom](whepfrom)

### Web WHIP/WHEP client

**Open your browser, enter the URL: `http://localhost:7777/`**

### Debugger

example: `http://localhost:7777/tools/debugger.html`

You can use this test WebRTC-SVC

### Single Page Player

example: `http://localhost:7777/tools/player.html?id=web-0&autoplay&controls&muted&reconnect=3000`

URL params:

- `id`: string, live777 Stream ID
- `autoplay`: boolean
- `controls`: boolean
- `muted`: boolean, whether to mute by default
- `reconnect`: number, reconnect timeout in millisecond

## Minimize Running

### Run Live777 using docker:

::: danger
**You must use network host mode**
:::

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

**Open your browser, enter the URL: `http://localhost:7777/`**

