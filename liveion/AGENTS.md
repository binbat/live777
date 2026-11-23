# liveion — Agent Guide

Deep-dive notes for the core SFU library behind the `live777` binary. The
root [AGENTS.md](../AGENTS.md) covers repo-wide conventions, build/test
commands and the workspace map — read it first. This file is derived from
the actual code; if something conflicts with the code, the code wins. When
you change behavior documented here, update this file.

## Module Map

- `src/route/` — Axum route handlers (whip, whep, session, admin, stream,
  source, recorder, info, sdp).
- `src/forward/` — SFU forwarding core (publish, subscribe, channel, track,
  bridge, media, RTCP); see "Forwarding Core" below.
- `src/stream/` — stream manager + source adapters; see "Stream Manager"
  below.
- `src/event.rs` — typed stream-lifecycle events (`stream_created` …
  `subscribe_stopped` with reasons) on a single manager-wide broadcast bus.
  Consumers must tolerate `broadcast::RecvError::Lagged` by continuing the
  loop (and re-snapshotting where applicable).
- `src/recorder/` — recording pipeline (fmp4, segmenter, uploader,
  codec-specific writers).
- `src/hook.rs` — stream-lifecycle hook scripts; see below.
- `src/target.rs`, `src/target_rtp.rs`, `src/target_rtsp.rs` — static WHIP /
  RTP / RTSP output targets; see below.

## Forwarding Core (`src/forward/`)

### Media statistics (issue #252)

Per-track and per-session counters live in `forward/stats.rs`
(`MediaStats`); hot paths only `inc()` them (publish read loop in
`track.rs`, subscriber write loop in `subscribe.rs`), and the manager's
`stats_tick` (2 s) `sample()`s them into bitrates plus monotonic stream
totals. Removal paths (`do_remove_publish_cleanup`,
`do_remove_subscribe_cleanup`, `remove_virtual_tracks`) fold each departing
flow's un-sampled tail into the totals, so counters stay exact across churn;
`info()` adds each live flow's `unsampled()` tail to the folded totals so
stream-level counters line up with the per-session ones between ticks;
removal also refreshes the aggregate bitrate immediately so closed
directions do not keep a stale rate until the next tick. Direct-tap
consumers (output targets, RTSP pull clients) count too (live777#474):
`Manager::add_virtual_subscriber` hands the consumer a `MediaStats` its send
loop `inc()`s, each send epoch registers it on the forward's tap list
(`PeerForward::add_tap_stats`, re-registered per epoch because a stream
reset recreates the forward), the tick samples taps into the outbound
totals, and detach or stream close folds the tail exactly once. Stats
surface as the `stats` field on the stream/session API types and as the
`live777_rtp_bytes_total{direction="in|out"}` Prometheus counter.

RTCP is counted separately as
`live777_rtcp_packets_total{direction="to_publisher|from_subscriber", kind="pli|fir|nack|rr|sr|twcc|remb|other"}`:
`to_publisher` comes from the publish chain's outermost interceptor
(`rtcp_egress_probe` in `forward/internal.rs`, which sees both
interceptor-generated and application-written packets, e.g. relayed
PLI/FIR), `from_subscriber` from the subscribe-quality tap
(`forward/subscribe_quality.rs`, compiled only with the `source` feature).

Per-stream series: `live777_stream_rtp_bytes_total{stream, direction}`
shares the RTP accounting points (tick deltas plus final folds), and
`live777_stream_sessions{stream, kind="publish|subscribe"}` is *set* from
the live session counts in the same stats tick — poll-model gauges that
cannot drift, unlike inc/dec counters. A stream's series are removed from
the vecs at teardown (`emit_stream_deleted`): the prometheus client never
drops vec children on its own, so without the removal deleted streams would
stay in /metrics forever (registry growth on churn, and Prometheus would
never mark them stale). The `stream` label is still unbounded on auto-create
deployments while the streams are *live*; the removal keeps the registry
bounded to live streams. The stream API includes `statsScope`: `node` for
liveion snapshots and `clusterNodeWork` for liveman's merged sum of per-node
work, where cascade hops are counted on each relay node.

Snapshot freshness is cadence-driven: `stats_tick` unconditionally bumps a
`watch` version that both SSE and the net4mqtt xdata notifier subscribe to,
and both dedup on the exact serialized payload (which covers stats) — live
rates push every tick while media flows, a silent stream's zero rate flushes
exactly once, and an idle server sends nothing.

### DataChannel lifecycle

Every stream owns a stream-level **cable hub** (`stream/cable.rs`, held by
`Manager::cable_hubs`) — an anonymous default bus pair plus one bus pair per
declared cable — so a provisioned-stream reset (which recreates the forward)
never churns the buses or the cable UDP bridges' listen ports; bridges die
with the hub (stream delete), via `Drop` cancelling a hub token. A peer
channel joins the cable matching its label (`CableHub::route`), else the
default pair. Every channel gets a read and a write task
(`dc_read_loop`/`dc_write_loop` in `forward/internal.rs`) bridging it to its
cable's buses; a bus send failure (no active receivers) drops the message
with a warn log instead of killing the loop. The webrtc driver never
delivers `OnClose` for a server-side `PeerConnection::close` (it aborts
first), so each session carries a `CancellationToken` (`dc_cancel` on
`PublishRTCPeerConnection`/`SubscribeRTCPeerConnection`, cancelled from
`Drop`) that both loops `select!` on. `on_data_channel` only wires a channel
whose peer still owns a live session (a late announce from a
replaced/torn-down session is dropped), and both sides gate sends on
`Connected` (`wait_for_peer_connected`, 15 s). Cables with `create = true`
get a server-created channel per session — created on the session's first
inbound channel OPEN (an inbound OPEN proves the SCTP association is
Established and the DTLS role resolved; creating at `Connected` drops the
OPEN write as non-Established and can collide stream-id parity with the
client's channels) — and a client-created channel matching such a cable's
label is closed (the server-created channel wins).

DC UDP endpoints are declared as `udp://` URLs on the stream's `sources`
(broadcast seat: inbound packets go to all subscribers, replies go to the
endpoint's `?host`/`?port` target) and `targets` (subscriber seat: publisher
messages go to the target, inbound packets go to the publisher), each a
`cables` entry carrying the cable `label` it joins plus a `udp://` URL,
attached once via `CableHub::attach_static_endpoints` (idempotent across a
provisioned-stream reset; `attach_source`/`attach_target`/`detach` are the
runtime foundation for a future dynamic API). The `udp://` authority
(listen) and the `host`/`port` query pair (target) are independently
optional (one-way endpoints; an omitted target means reply-to-sender). The
client side (whepfrom/whipinto channel bridges in `livetwo::datachannel`)
mirrors this — see [livetwo/AGENTS.md](../livetwo/AGENTS.md).

Every session additionally keeps a label → open-channel registry
(`channels` on `PublishRTCPeerConnection`/`SubscribeRTCPeerConnection`),
populated in `publish_data_channel`/`subscribe_data_channel` when a channel
is actually bridged (after the server-created dedup close). It backs the
HTTP injection API `POST /api/streams/{stream}/channels/{label}/messages`
(admin route, JWT scope `x`): without `?session=` the manager broadcasts
onto `CableHub::route(label).to_subscribers` (declared cable or the
anonymous default pair, so any label works) and reports the live receiver
count minus the permanent drain task; with `?session=` it sends
`send(BytesMut)` directly onto that session's registered channel
(`PeerForwardInternal::cable_channel`), evicting the stale entry and
returning 404 when the direct send fails. Injected frames are binary on the
wire (the bus carries `Vec<u8>`).

Both loops feed the DataChannel Prometheus metrics directly (no stats tick —
messages are low-volume discrete events, the same pattern as the RTCP
probe): `live777_datachannel_messages_total{direction="in|out"}` and
`live777_datachannel_bytes_total{direction}` count at receipt
(`dc_read_loop`) resp. after a successful channel write (`dc_write_loop`, so
bus fan-out is counted per attached peer), bus-lag drops land in
`live777_datachannel_dropped_total{direction="out"}`, and the per-stream
`live777_stream_datachannel_bytes_total{stream, direction}` series are
removed at stream teardown (`emit_stream_deleted`) like the RTP ones.

## Stream Manager (`src/stream/`)

### Provisioned streams

Every `[stream.<name>]` config entry is *provisioned*: pre-registered at
startup (`Manager::provision_streams`), always listed in the API/Dashboard,
exempt from orphan/auto-delete reapers, and rejected (409) on admin API
create/delete. Internal teardowns (`Manager::teardown_stream`, used by RTSP
re-ANNOUNCE and session cascades) reset a provisioned stream to standby with
a `StreamDeleted`+`StreamCreated` pair instead of removing it.

### On-demand sources and virtual subscribers

With `on_demand = true` the stream's sources start on the first subscriber
(WHEP/cascade push/RTSP pull) and stop `on_demand_close_after_ms` after the
last one leaves; source start/stop emits `PublishStarted`/`PublishStopped`
with the synthesized `virtual-source` session id. On-demand readiness is
judged by the source *bridge* (`SourceManager::has_bridge`), not source
existence, and starts/stops serialize on a per-stream lock
(`on_demand_locks`). The idle check (`on_demand_stream_idle`) counts real
subscriber sessions (WHEP/cascade) and registered *virtual subscribers*
(`Manager::virtual_subscribers`): internal consumers that tap the forward
track broadcast directly instead of holding a subscribe session — static
targets and RTSP pull clients. This is the subscribe-side counterpart of the
`virtual-source` publisher: attach/detach
(`add_virtual_subscriber`/`remove_virtual_subscriber`) emits
`SubscribeStarted`/`SubscribeStopped`, `Manager::info`/`do_snapshot`
synthesize always-Connected sessions with reserved `virtual-…` ids (e.g.
`virtual-target-rtp-230.1.1.2:1720`, so dashboards show why an on-demand
stream is running), the per-stream
`live777_stream_sessions{kind="subscribe"}` gauge and the global
`live777_subscribe` gauge include them, and the session-delete API rejects
their ids (400 — they are owned by their consumer's supervisor). Without the
registration the idle check stopped an on-demand stream's sources every
`on_demand_close_after_ms` underneath its targets (live777#481). The
recorder is intentionally not registered: it must not keep an on-demand
source alive by itself.

### Publisher conflicts and override

A WHIP publish onto a stream with an active source bridge is rejected (409)
to avoid mixing two publishers' tracks. A second WHIP publish on an
already-published stream instead *displaces* the incumbent (mediamtx-style
override): the old session is torn down with
`PublishStopped`/`SessionStopReason::Replaced`, and same-codec takeovers are
seamless to subscribers via the media-generation machinery. Streams opt out
with `strategy.override_publisher = false` (global or per-stream), restoring
the 409 — except a plain-WHIP incumbent that is already
`Disconnected`/`Failed`/`Closed` is displaced even then, so a fast reconnect
does not bounce off the zombie session with a 409 while the
disconnected-watchdog is still reaping it. Cascade-pull publishers are never
displaced and always conflict (409), since their supervisor would reconnect
and fight.

### Source startup and retry semantics

Configured sources retry independently in the background without delaying
HTTP; on-demand starts use their configured timeout, and shutdown cancels
retries. Subscriptions to a configured stream whose source bridge is pending
wait for a bounded startup period and fail as not ready if it is still
unavailable, rather than creating a trackless session. Cable-only
configurations do not wait for a media bridge. A pending source start
rejects competing WHIP publishers and has its own cancellation token, so
removal and `stop_all` interrupt its retry wait; bridge creation also
rejects a live publisher rather than mixing real and virtual tracks. After a
successful start, capture retries recovery/reopen with bounded waits.

### Source encoder bitrate control (issue #409)

Adaptive bitrate is on by default for native encoder sources (`adaptive =
false` opts out, `adaptive = { min_bitrate = … }` sets a custom floor; the
default floor is the lowest tier's bitrate when tiers are declared, else
`max(bitrate / 8, 300k)`). The AIMD controller
(`stream/source/adaptive_bitrate.rs`) retunes the running encoder from WHEP
subscriber RTCP feedback (sampled by `forward/subscribe_quality.rs`);
runtime retuning needs encoder-backend support (`EncoderBackend::setBitrate`
in livehal — rkmpp, v4l2-m2m and cedar today). Two admin surfaces sit on
top, deliberately separate: `GET /api/sources/{stream}/bitrate` is read-only
encoder telemetry (drive mode `adaptive`/`fixed` plus the current rate),
while `GET`/`POST /api/sources/{stream}/tier` is the control surface — a
source-level ladder (`stream/source/tier.rs`): bitrate-only tiers retune in
place, and a tier carrying `width`/`height`/`fps` switches the whole rung by
rebuilding the video capture+encoder pipeline
(`NativeEncodedSource::reconfigure` — the RTP broadcast channels survive, so
subscribers stay attached across the sub-second gap; a failed rebuild rolls
back to the previous video params). An audio leg, when configured, keeps its
pipeline and RTP packetizer running across video rebuilds and rollbacks,
preserving its SSRC and timestamp base. A per-stream `BitrateControl`
(created for every native source in `create_bridge`) coordinates tier
applies with the controller: the tier's bitrate becomes the AIMD's rung
ceiling and resume seed without suspending it, and an external-change
generation counter guarantees the controller re-seeds even for applies
shorter than its 1 s tick.

### Native source audio leg

Native sources may be audio-only or video+audio: the audio leg is
`sine`/ALSA capture → opus-rs encode (`livehal::AudioPipeline`) → the
`NativeEncodedSource` audio packetize task → `MediaPacket::AudioRtpPacket` →
`PeerForward::inject_audio_rtp_packet` (fixed wire contract: Opus PT 111,
48 kHz, fmtp `minptime=10;useinbandfec=1`). Initial ALSA open attempts fail
synchronously and are retried by the source manager. Opus RTP timestamps
advance by the encoded frame's sample count, not capture-read wall-clock
deltas. Tiers and the adaptive controller target the video encoder when both
legs exist (the audio leg's own `adaptive` key is ignored); on an audio-only
source they drive the Opus encoder.

Test coverage: `tests/native_audio.rs` (gated on
`native-source,encoder-opus`) is the end-to-end path; unit tests cover retry
budgets/cancellation and an injected read failure followed by ALSA reopen
and bounded stop, and an ignored test in
`src/stream/source/native_encoded_source.rs` checks video
reconfigure/rollback while preserving the sine audio SSRC.

## Wayland Compositor Source (`source-wayland`, Linux-only)

URL form: `wayland://<socket>?width=&height=&fps=&bitrate=`; the code lives
in `src/stream/source/wayland/{mod,compositor,encoder,convert,pattern,gl}.rs`.
Build prerequisites: the `libxkbcommon` shared library to link against
(`libxkbcommon.so`, e.g. the `libxkbcommon-dev` package on Debian/Ubuntu;
pulled in by smithay's seat keyboard handling — no headers needed, the crate
carries its own FFI bindings), plus `libpipewire-0.3-dev` and `libopus-dev`
for the desktop audio capture. Runtime: an `Xwayland` binary for X11
clients, and a PipeWire daemon (+ session manager) for audio.

A smithay headless compositor runs on a dedicated calloop thread (no GPU
output; wl_compositor/wl_shm/xdg-shell/wl_output/wl_seat/data-device
globals, plus `zwp_linux_dmabuf_v1` when GL is up) and composites visible
xdg toplevels onto a fixed-size canvas, emitting frames only when a commit
made the canvas dirty (plus PLI-repaint and a 2 s keepalive so late-joining
subscribers always get an IDR).

Compositing has two paths (`gl.rs` vs the CPU blit): when a GPU/EGL renderer
initializes (GBM device on `/dev/dri/renderD128` → `card0`,
`LIVE777_WAYLAND_DRM_DEVICE` overrides; EGL + `GlesRenderer`; a probe
render+readback must succeed), every toplevel buffer — dmabuf via
`ImportDma`, SHM via `ImportMemWl` — is imported as a texture and drawn into
one persistent offscreen target, then read back through
`ExportMem::copy_framebuffer` (rendered `Flipped180` so the bottom-up
readback lands top-down, BGRA so the byte order matches the canvas); without
a renderer the CPU alpha-over SHM blit handles everything and no dmabuf
global is advertised. GL state is `!Send` and lives on the event-loop thread
inside the frame timer; `dmabuf_imported` can only sanity-check against the
advertised format set (protocol dispatch never touches the renderer), and
the real import happens per frame.

Frames cross into the encode pipeline as `ArgbFrame` over an mpsc channel —
the seam between frame producer and encoder: a synthetic pattern generator
feeds it until the compositor's first contentful frame, then a `frame_mux`
task permanently switches over (an all-clients-disconnected desktop keeps
streaming the background canvas; the pattern never resumes). The pipeline
converts XRGB8888→I420 (`yuvutils-rs`, BT.601 limited) and encodes with
`openh264` (`UsageType::ScreenContentRealTime`, in-band SPS/PPS ahead of
every IDR via `SpsPpsStrategy::IncreasingId`, periodic IDR ≈ every 2 s).
PLI/FIR from subscribers forces an IDR (`force_intra_frame`) and nudges a
repaint; the crate has no runtime bitrate API, so `set_bitrate` recreates
the encoder (sub-ms, the fresh encoder starts with an IDR). The source
deliberately does not opt into the AIMD adaptive controller. The compositor
socket is created under `$XDG_RUNTIME_DIR` — the source fails to start
without it.

X11 clients are supported via XWayland (`xwayland=1` default, `xdisplay=N`
pins `DISPLAY`, needs an `Xwayland` binary on PATH — skipped with a warning
without one): the compositor spawns the server, and a minimal built-in XWM
(`XwmHandler` in `compositor.rs`) grants configure requests as-is, maps
fullscreen/maximize to the canvas, keeps last-mapped on top, and activates
only the top window (no input injection yet). The canvas stack is xdg
toplevels, then managed X11 windows, then X11 override-redirects on top; X11
windows draw 1:1 at their root-relative position (no CSD offsets), xdg
windows stay centered (`canvas_window_pos`). X11 buffers ride the same
SHM/dmabuf paths (`canvas_windows()` unifies the two kinds), and
`CompositorHandler::client_compositor_state` must special-case the XWayland
client's `XWaylandClientData`. Shutdown caution: calloop source closures
capturing a `LoopHandle` Rc-cycle the registry (smithay's X11Source does
this internally), so `run()` explicitly removes every source it inserted at
exit — dropping the listener frees the socket file, dropping the XWayland
handle kills the server — while one registry skeleton (epoll fd + X11
channel) still leaks per compositor lifetime (smithay-0.7 limitation;
bounded because restarts are rare). X11 clients that paint before Xwayland
realizes their window lose that early damage — they must repaint on Expose
(well-behaved toolkits do). `LIVE777_WAYLAND_XWAYLAND_LOG=1` inherits the
Xwayland stderr for debugging.

Desktop audio (`audio=1` default, `audio_bitrate=`, `audio_sink=` name
filter) is PipeWire capture (`wayland/audio.rs`): one thread runs a PipeWire
main loop, handshakes synchronously at `start()` (daemon unreachable →
video-only with a warning), taps the monitor of an `Audio/Sink` node
(session manager such as wireplumber must be running to link it), encodes
20 ms Opus frames and broadcasts `MediaPacket::AudioRtpPacket` — the
pre-packetized audio variant shared with the native source's audio leg,
which the bridge injects into the audio track via
`PeerForward::inject_audio_rtp_packet` (`RtpPacket` stays video).
`get_audio_codec` is gated on the video codec being known because the bridge
adds tracks in a single pass once any codec is ready.

Remote input (`input=1`, off by default — every subscriber can then drive
the desktop): browser-side JSON events (`web/player-core/input-capture.ts`,
`WhepPlayer`'s `input` prop, standalone player `&input`) ride the stream's
`input` datachannel cable; the bridge pumps
`cables.route("input").to_publisher` into the source's
`StreamSource::input_sink()` (a raw-bytes closure so the trait stays
format-agnostic), `wayland/input.rs` parses them, and a calloop channel
crosses into the compositor thread where `handle_input` injects into the
seat: hit-tested pointer focus over the unified window stack,
focus-follows-click keyboard focus, X11 windows raise+activate on click.
DomCode→evdev translation is a static table in `input.rs` (xkb keycodes are
evdev+8 at the smithay boundary).

End-to-end tests: `tests/wayland_source.rs` (Linux, `source-wayland`; the
client-content test additionally needs `rsmpeg` — a real Wayland client
paints a solid-color toplevel, and the WHEP subscriber depacketizes +
decodes the pattern→client switch and the background after the client
disconnects).

## Lifecycle Hooks (`src/hook.rs`)

Stream-lifecycle hook scripts (`[hooks]` global + `[stream.<name>.hooks]`
per stream) run by a single FIFO executor: dispatcher forwards
`StreamCreated`/`StreamDeleted`/`PublishStarted`/`PublishStopped` into an
internal queue, then scripts run sequentially (global first, per-stream
after, configured order) with per-script timeout and `on_error` policy. One
hook deliberately bypasses the queue: `on_source_changed` scripts run
**synchronously inside** a source's parameter-set change (the admin tier
API, `apply_source_tier`) before the capture+encoder re-provisioning — for
hardware that must be reconfigured ahead of the pipeline rebuild (e.g.
camera sensor mode gears on Rockchip-style V4L2 pipelines, where framerate
is a sensor-mode property). A hook failure aborts the apply when `on_error =
"stop"` (`ApplyTierOutcome::HookFailed` → 500). When the apply does not
reach the target state (aborted, or the rebuild failed and rolled back), the
scripts run again with the source's *current* state
(`StreamSource::active_tier_state`, best effort) so hardware they switched
is switched back. Scripts see argv `<stream> <tier>` and
`LIVE777_SOURCE_TIER` plus the source geometry/bitrate
(`LIVE777_SOURCE_WIDTH`/`_HEIGHT`/`_FPS`/`_BITRATE`) — declared tier values
on the pre-apply run, the pipeline's actual current values on the
compensation run.

## Static Output Targets

### WHIP push targets (`src/target.rs`, `target-whip`)

Static WHIP push targets (`[[stream.<name>.targets]]`, declarative
cascade-push). One supervisor task per target keeps the push media-driven:
established on `PublishStarted`, torn down on `PublishStopped` (the push
negotiates per media epoch, so its codecs always match the current
publisher), retried with source-style backoff (5 s doubling, 60 s cap),
reconciled against the manager on event-bus lag; a target on an `on_demand`
stream is standing demand: the supervisor registers as a virtual subscriber
(`Manager::add_virtual_subscriber`, id `virtual-target-…`) for its whole
lifetime so the sources are never idle-stopped underneath it, and (re)starts
them whenever the stream has neither a publisher nor a push session, paced
by the same backoff. `target::init` dispatches per target by URL scheme, so
`whip://`, `rtp://` and `rtsp://` targets mix freely on one stream.

### RTP/UDP targets (`src/target_rtp.rs`, `target-rtp`)

Static RTP/UDP output targets (`rtp://group-or-host:port`), the sender
counterpart of the SDP-file source: live777 acts as the multicast sender,
keeping Unitree-style video-link receivers working unmodified. The
supervisor mirrors the WHIP one (media-driven, same backoff/reconcile/
standing-demand semantics) plus per-track send tasks that tap the forward
track broadcast directly (the recorder / RTSP-server pattern, no subscribe
session): first video track to the URL port, first audio track to port + 2
(RTP/AVP convention). Like the RTSP target, the epoch waits for the
*negotiated* publish track counts (`PeerForward::wait_for_publish_tracks`,
shared by both media targets, on top of
`PeerForwardInternal::negotiated_publish_track_counts`) instead of the first
non-empty snapshot — tracks arrive one `on_track` each after
`PublishStarted`, so a snapshot would race an AV publisher into a video-only
send and SDP. Send tasks report their exit tagged with the epoch's
generation, and the supervisor ignores tags that are not the live epoch — a
belated exit from an already torn-down epoch must never cancel the fresh
one, or every teardown self-sustains a restart storm. A live epoch's
spontaneous exit (and a reconcile after event lag) is paced like the WHIP
supervisor's dead-session path: tear down, wait `reconnect_delay(1)` while
media is still up, re-check, then rebuild — a flapping track set cannot
rebind the socket, rewrite the SDP file and PLI the publisher at the churn
rate (live777#477). While media flows, each send task also emits a periodic
RTCP sender report to its RTP port + 1 (packet/octet counters and the NTP ↔
RTP timestamp mapping are maintained at the marshal point, live777#471);
inbound receiver reports are not consumed. Video is re-packetized through
livetwo's `RePayloadCodec` (SPS/PPS inlined ahead of every IDR, seeded from
the codec fmtp via `with_sprop_params`), a PLI nudges the publisher on
attach, and the video payload type can be pinned (`payload_type`, e.g. 96
for Unitree receivers); `sdp_file` writes a receiver-side SDP on send start
that live777's own SDP-file source can ingest (live777 → live777 multicast
cascade). The multicast socket builders are shared in
`livetwo::transport::multicast` — see
[livetwo/AGENTS.md](../livetwo/AGENTS.md).

### RTSP push targets (`src/target_rtsp.rs`, `target-rtsp`)

Static RTSP client-push output targets
(`rtsp://[user:pass@]host:port/path[?transport=tcp|udp]`), the counterpart
of the RTSP source: live777 ANNOUNCE/SETUP/RECORDs the stream to an RTSP
server (mediamtx, another live777's RTSP server, gst-rtsp-server) with the
`libs/rtsp` client whepfrom also uses. The supervisor mirrors the RTP one
(media-driven, same backoff/reconcile/standing-demand semantics,
generation-tagged epochs) and the media plane is the same track-broadcast
tap + `RePayloadCodec` + payload-type re-stamp; the ANNOUNCE SDP is built
from the forward's track codecs in the shape of the RTSP server's DESCRIBE
(`a=control:` per media, which the client session derives the SETUP URLs
from). `PublishStarted` fires at negotiation time while tracks arrive one
`on_track` each, so the epoch waits for the *negotiated* track counts
(`PeerForward::wait_for_publish_tracks`, shared with the RTP target) instead
of the first non-empty snapshot — an AV publisher whose audio track lands
first would otherwise announce audio-only. UDP is the default transport
(`?transport=tcp` selects interleaved, mirroring whepfrom); UDP senders bind
the SETUP-announced local port because strict servers (mediamtx) drop RTP
from any other source port. Teardown cancels the epoch token threaded into
`rtsp::setup_rtsp_session`: TCP closes the connection, UDP sends TEARDOWN
from the keep-alive loop, so the server releases the published path
immediately and a re-push cannot collide with a zombie publisher.
Server-side keyframe requests (a puller's PLI/FIR relayed by mediamtx or
another live777) arrive on the interleaved RTCP channel / UDP RTCP port and
are forwarded to the publisher via `send_rtcp_to_publish`.
