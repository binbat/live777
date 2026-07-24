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
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]` (empty string disables ICE servers) |
| `-v` | `warn` | Increase verbosity (`-v` info, `-vv` debug, `-vvv` trace) |

## RTP

RTP mode need `target` and `sdp file`

```bash
whepfrom -o rtp://{target_ip}?video={video_port}&audio={audio_port} -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

```bash
whepfrom -o rtp://localhost?video=9000&audio=9002 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

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

