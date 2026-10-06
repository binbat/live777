# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

This tool has two working mode:
- `rtp`
- `rtsp as client`

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-o`, `--output` | `sdp://0.0.0.0:8555` | Output target: `rtp://` / `rtsp://` / `sdp://` |
| `-w`, `--whep` | required | WHEP endpoint URL |
| `--sdp-file` | `output.sdp` | SDP filename to write (RTP mode) |
| `-t`, `--token` | none | Bearer token for WHEP authentication |
| `--command` | none | Run a command as child process |
| `--channel` | none | DataChannel &lt;-&gt; UDP forwarding URL, e.g. `udp://0.0.0.0:9001?host=127.0.0.1&port=9000` |
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

`whepfrom` as a client, push stream from RTSP Server

```bash
whepfrom -w http://localhost:7777/whip/777 -o rtsp://127.0.0.1:8554
```

### Use transport `tcp`

```bash
whepfrom -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/test-rtsp?transport=tcp
```

