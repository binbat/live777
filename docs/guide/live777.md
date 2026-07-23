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

Configure the listen URL in `live777.toml`.  When credentials are present in
the URL, Digest authentication is enabled automatically:

```toml
[rtsp]
# No auth:
listen = "0.0.0.0:8554"
# With Digest auth:
# listen = "rtsp://admin:secret@0.0.0.0:8554"
```

The same port handles both directions:

- Push media into a stream: `rtsp://host:8554/{stream_id}` (`ANNOUNCE` + `RECORD`)
- Pull media from a stream: `rtsp://host:8554/{stream_id}` (`DESCRIBE` + `PLAY`)

Both UDP and TCP (`RTP/AVP/TCP`) transports are supported.  The first URL path
segment is used as the liveion stream identifier.

## Stream Hooks

Live777 can run external scripts on stream lifecycle events: stream
creation/deletion, and publish start/stop (a WHIP/cascade publisher
attaching, or a configured source starting/stopping). A typical use is
on-demand source activation for devices live777 cannot drive directly: a
hook starts a capture device / hardware encoder when media is actually
wanted, and stops it when the last consumer leaves to save resources.

> Built-in alternative: configured sources (`[[stream.<name>.sources]]`)
> support `on_demand = true`, which starts/stops the camera or RTSP pull
> automatically with subscriber presence — no scripts needed. See
> [livehal — Provisioned streams and on-demand sources](./livehal#provisioned-streams-and-on-demand-sources).

> Which events signal "someone is watching"? For **provisioned** streams
> (any `[stream.<name>]` entry) `stream-created` fires once at startup, so
> it cannot drive power control — use `publish-started` / `publish-stopped`
> instead. Configured sources report their start/stop as publish events
> with the session id `virtual-source`. For **dynamic** streams
> (`auto_create_whep`), `stream-created` still coincides with first use.

```toml
# Global hooks, run for every stream.
[hooks]
timeout_ms = 5000    # per-script timeout, 0 disables
on_error = "stop"    # "stop" skips the remaining hooks of the same event;
                     # "continue" runs them anyway
on_stream_created = ["/etc/live777/hooks/notify.sh"]
on_stream_deleted = ["/etc/live777/hooks/notify.sh", "/etc/live777/hooks/cleanup.sh"]
on_publish_started = ["/etc/live777/hooks/camera-power.sh"]
on_publish_stopped = ["/etc/live777/hooks/camera-power.sh"]

# Per-stream hooks, run after the global ones.
[stream.cam1.hooks]
on_stream_created = ["/etc/live777/hooks/cam1-created.sh"]
on_stream_deleted = ["/etc/live777/hooks/cam1-deleted.sh"]
on_publish_started = ["/etc/live777/hooks/cam1-power-on.sh"]
on_publish_stopped = ["/etc/live777/hooks/cam1-power-off.sh"]
```

Execution guarantees:

- Hooks of one event run sequentially: global first, then per-stream, in
  configured order.
- Events are processed in a single FIFO queue — all hooks of an earlier
  event finish before any hook of a later event starts, so a stream's
  `stream-created` hooks always complete before its `stream-deleted` hooks begin.
- A failed script (non-zero exit, spawn error, timeout kill) is logged and
  handled per `on_error`; it never affects later events or the server.

Scripts are executed directly (no shell) and receive the event metadata both
as argv and as environment variables:

| argv                | env               | value                                                                                     |
| ------------------- | ----------------- | ----------------------------------------------------------------------------------------- |
| `$1`                | `LIVE777_EVENT`   | `stream-created` / `stream-deleted` / `publish-started` / `publish-stopped`               |
| `$2`                | `LIVE777_STREAM`  | stream name                                                                               |
| `$3` (deleted/stopped only) | `LIVE777_REASON` | stream-deleted: `api-deleted` / `publish-leave-timeout` / `subscribe-leave-timeout` / `orphaned` / `reset`; publish-stopped: `peer-closed` / `api-deleted` / `idle-timeout` |
| —                   | `LIVE777_SESSION` | (publish events only) publisher session id; `virtual-source` for configured sources       |

Notes:

- Scripts should return quickly after initiating their work (e.g. launch an
  encoder in the background). A blocked script blocks the whole hook queue.
- Make scripts idempotent: a `stream-deleted` hook also runs when the publisher
  itself died (`publish-leave-timeout`), so stop scripts must tolerate an
  already-stopped device.
- For on-demand sources, combine with `[strategy] auto_create_whep = true`
  and a generous `auto_delete_whep` (e.g. `30000`) so brief subscriber
  flapping does not cycle the hardware. Per-stream overrides live under
  `[stream.<name>.strategy]`. Streams configured with `on_demand = true`
  already debounce via `on_demand_close_after_ms`.
- No hooks fire on server shutdown (no `stream-deleted` events are emitted
  then).

