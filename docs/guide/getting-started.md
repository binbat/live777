---
description: >-
  Start Live777, run the end-to-end smoke test, and verify an FFmpeg-to-ffplay
  media loop in three terminals.
---

# Getting Started

The fastest way to verify a Live777 deployment end to end is the all-in-one
`live777-all` image. It bundles `live777`, `liveman`, FFmpeg and the
WHIP/WHEP tools (`whipinto`, `whepfrom`, `whipsynth`, `whepprobe`), plus a
`smoke-test` command that checks a full publish → subscribe → decode loop.

## Start the server

::: danger
**You must use network host mode**
:::

```sh
docker run -d --name live777 --rm --network host ghcr.io/binbat/live777-all:latest
```

The default command is `live777`. **Open your browser, enter the URL:
`http://localhost:7777/`**

## Run the smoke test

```sh
docker exec live777 smoke-test
```

`smoke-test` publishes a synthetic test stream over WHIP, subscribes to it
over WHEP, and decodes the received audio and video with FFmpeg — proving the
whole `WHIP → live777 → WHEP` media path works, server included.

**Success**: the command prints a `whepprobe` JSON report (codec, frames
decoded, timing) and exits `0`.

**Failure**: the exit code is non-zero and the output prints diagnostics —
which leg failed (WHIP publish, WHEP subscribe, decode) and why. Check the
server log first (`docker logs live777`).

## Notes: tokens, remote clients, ICE

- If the server requires a Bearer token (`[auth] tokens` in
  `live777.toml`), pass it to the smoke test via the `SMOKE_TOKEN`
  environment variable:

  ```sh
  docker exec -e SMOKE_TOKEN=<token> live777 smoke-test
  ```

- To check a **remote** live777 instead of the container's own, point
  `SMOKE_SERVER_URL` at it — no server needs to run in the container:

  ```sh
  docker run --rm --network host -e SMOKE_SERVER_URL=http://<server>:7777 \
    ghcr.io/binbat/live777-all:latest smoke-test
  ```

- CI and troubleshooting can also tune `SMOKE_STREAM`, `SMOKE_ATTEMPTS` and
  `SMOKE_READY_TIMEOUT`; the defaults are a unique stream, five probe
  attempts and a 30-second server readiness timeout.

- The smoke test runs entirely inside the container against the local
  server, so it validates the server itself. Clients on **other machines**
  additionally need working ICE: the default configuration has no ICE
  servers (host candidates only, LAN-only). Configure `[[ice_servers]]` /
  `[webrtc] ice_udp_addrs` in `live777.toml` for remote or NAT-crossing
  clients — see the [live777 configuration guide](/guide/live777).

## Stop and clean up

```sh
docker stop live777
```

The `--rm` flag removes the container automatically.

## Standalone variant

If no server is running yet, a single command starts a temporary server,
runs the check and tears everything down:

```sh
docker run --rm --network host ghcr.io/binbat/live777-all:latest smoke-test --standalone
```

Standalone mode uses an isolated temporary configuration and loopback-only
ICE. Set `SMOKE_PORT` to change its HTTP port (default `7777`).

## Verify the RTP bridge in three terminals

For an authoritative local `FFmpeg → WHIP → WHEP → ffplay` loop, install
`live777`, `whipinto`, `whepfrom`, FFmpeg and ffplay (see
[installation](/guide/installation)). Stop the Docker server first if it is
using port 7777, then run one command in each terminal from the same working
directory.

**Terminal 1 — start the SFU:**

```sh
live777
```

**Terminal 2 — generate H264 RTP and publish it with whipinto:**

```sh
whipinto -i input.sdp -w http://127.0.0.1:7777/whip/777 --command \
  "ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 \
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \
  -f rtp rtp://127.0.0.1:5002 -sdp_file input.sdp"
```

**Terminal 3 — subscribe with whepfrom and play the generated SDP:**

```sh
whepfrom -w http://127.0.0.1:7777/whep/777 \
  -o rtp://127.0.0.1:9000 --sdp-file output.sdp --command \
  "ffplay -protocol_whitelist rtp,file,udp -i output.sdp"
```

The RTP output URL sends video to port 9000 (audio, when present, goes to
9002); `--sdp-file` describes that receiver to ffplay. Stop each terminal
with Ctrl-C when finished. See [whipinto](/guide/whipinto),
[whepfrom](/guide/whepfrom) and the [FFmpeg guide](/guide/ffmpeg) for tokens,
other codecs and RTSP variants.

## What the smoke test does not cover

`smoke-test` verifies the `WHIP → live777 → WHEP` audio/video path with FFmpeg
decoding. Beyond it:

- To verify **browser** rendering, use [WhepWright](/guide/whepwright),
  which plays the stream in a real browser driven by Playwright.
- To bridge **RTP/RTSP** sources or outputs, use
  [whipinto](/guide/whipinto) and [whepfrom](/guide/whepfrom) — they are
  protocol bridges, not the default verification path.
