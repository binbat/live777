---
description: >-
  Bridge a Live777 WHEP stream to RTP or RTSP, write receiver SDP, and
  diagnose authentication, media, and ICE failures.
---

# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

This tool has two working modes:
- `rtp`
- `rtsp as client`

Use [whipinto](/guide/whipinto) for the reverse RTP/RTSP → WHIP direction, or
follow the [three-terminal loop](/guide/getting-started#verify-the-rtp-bridge-in-three-terminals).

## Get it and prerequisites

Download a package or prebuilt binary using the
[installation guide](/guide/installation), or build from this repository with
`cargo build --release --bin whepfrom`. You need a reachable Live777 WHEP
endpoint with an active publisher. RTP playback also needs a receiver such as
ffplay or VLC; RTSP output needs a running RTSP server.

## Bearer token

If `live777.toml` contains `tokens = ["live777"]` under `[auth]`, pass the
same token with `-t`:

```bash
whepfrom -t live777 -w http://127.0.0.1:7777/whep/777 \
  -o rtp://127.0.0.1:9000 --sdp-file output.sdp
```

Do not put tokens in URLs or commit them to shell history and configuration
files intended for publication.

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-o`, `--output` | `sdp://0.0.0.0:8555` | Output target: `rtp://` / `rtsp://` / `sdp://` |
| `-w`, `--whep` | required | WHEP endpoint URL |
| `--sdp-file` | `output.sdp` | SDP filename to write (RTP mode) |
| `-t`, `--token` | none | Bearer token for WHEP authentication |
| `--command` | none | Run a command as child process |
| `--cable-url` | none | Cable UDP bridge URL for DataChannel &lt;-&gt; UDP forwarding, e.g. `udp://0.0.0.0:9001?host=127.0.0.1&port=9000` |
| `--cable-label` | `-` | DataChannel label; joins the matching cable on the server |
| `--ice-server` | none | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]`. Defaults to none — host candidates only (WHIP/WHEP endpoints advertise their own ICE servers via Link headers) |
| `-v` | `warn` | Increase verbosity (`-v` info, `-vv` debug, `-vvv` trace) |

## RTP

RTP mode need `target` and `sdp file`

```bash
whepfrom -o rtp://{target_ip}?video={video_port}&audio={audio_port} -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

```bash
whepfrom -o rtp://localhost?video=9000&audio=9002 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

The URL's own port also works: video goes to it and audio to port + 2 (the
RTP/AVP convention), while `?video=`/`?audio=` still override per track:

```bash
whepfrom -o rtp://localhost:9000 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

A multicast group as the host makes whepfrom a self-contained WHEP →
multicast bridge — the tool-side counterpart of live777's `rtp://` output
target:

```bash
whepfrom -o rtp://230.1.1.1:1720 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

Multicast options ride as query parameters: `?ttl=16` (IPv4 TTL / IPv6 hops,
default 1) and `?interface=...` (an IPv4 interface address for IPv4 groups,
an interface index or name for IPv6 groups). Both are rejected for unicast
destinations.

Use [`ffplay`](/guide/ffmpeg) play

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

Use [`vlc`](/guide/vlc) play

```bash
vlc output.sdp
```

## RTSP from live777

The `rtsp-listen` (whepfrom as RTSP server) mode was removed. live777 has a
built-in RTSP server for every stream — pull directly instead:

```bash
ffplay rtsp://localhost:8554/777
```

### Use transport `tcp`

```bash
ffplay rtsp://localhost:8554/777 -rtsp_transport tcp
```

## RTSP Client

`whepfrom` as a client, push the stream to an RTSP server

```bash
whepfrom -w http://localhost:7777/whep/777 -o rtsp://127.0.0.1:8554/777
```

### Use transport `tcp`

```bash
whepfrom -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/test-rtsp?transport=tcp
```

## Troubleshooting

Run with `-vv` to include WebRTC and bridge diagnostics.

| Symptom | What to check |
|---------|---------------|
| `401 Unauthorized` | Pass `-t <token>` and make sure it exactly matches the server's `[auth] tokens`; do not add a second `Bearer` prefix. |
| `403 Forbidden` | Authentication reached the server but a proxy or cluster policy rejected it; check Live777/liveman authorization and routing rules. |
| `404 Not Found` | Check the `/whep/<stream>` URL, stream name, server port and reverse-proxy path. Make sure the publisher created the stream before subscribing. |
| `409 Conflict` | A session or resource conflict was rejected; inspect the server log, remove stale sessions and check subscriber limits or stream policy before retrying. |
| Connected, but no media | Confirm the stream has an active publisher and negotiated tracks. For RTP, use `-o rtp://127.0.0.1:9000 --sdp-file output.sdp` (not a bare SDP output), open that SDP in the receiver, and check UDP firewall/port rules. |
| ICE timeout or disconnected | Host candidates alone work only where the client and server can directly reach each other. For remote/NAT use, configure server ICE addresses and STUN/TURN, then pass `--ice-server` when the client needs it; see [live777 configuration](/guide/live777). |

