# Live777 — Agent Guide

This document is a concise orientation for AI coding agents working on the
Live777 repository. It is derived from the actual project files; if something
conflicts with the code, the code wins.

## Project Overview

Live777 is a lightweight, high-performance WebRTC SFU (Selective Forwarding
Unit) that uses the `WHIP`/`WHEP` protocols as its primary interface. It is
designed for real-time audio/video streaming and interoperates with clients
such as GStreamer, FFmpeg, OBS Studio, VLC, and browsers.

The repository is a mixed Rust + TypeScript/Vue project. Rust provides
the media server, protocol conversion, and command-line tools. TypeScript/Vite
provides the embedded WebUIs.

- Repository: <https://github.com/binbat/live777>
- License: `MPL-2.0`
- Authors: BinBat Ltd <hey@binbat.com>
- Contributors must sign the CLA in `.github/CLA.md` before submitting work.

## Technology Stack

- **Rust** — edition 2024, workspace version `0.9.0`.
- **Async runtime** — Tokio.
- **HTTP/API layer** — Axum, `tower-http` (CORS, tracing).
- **WebRTC stack** — `webrtc`/`rtc-*` crates, release `0.20.1` from
  crates.io.
- **Web UI** — Vite, Vue 3, Tailwind CSS, DaisyUI, TypeScript.
- **Package manager** — pnpm 10.20.0 (workspace covers `web/*`).
- **Storage** — OpenDAL for object/FS storage; Sea-ORM + SQLite (or Postgres)
  in `liveman` for recording indexes.
- **Media testing** — FFmpeg and GStreamer pipelines (see `justfile`).
- **Task runner / local recipes** — `just` (`justfile`).

## Workspace Layout

The root `Cargo.toml` defines a workspace with these members:

```
.                    # root crate, produces several binaries
libs/api             # shared REST/WebRTC request/response types
libs/auth            # JWT + static-token auth middleware
libs/cli             # shared CLI helpers (SDP parsing, shellwords)
libs/http-log        # Axum request/response logging middleware
libs/iceserver       # STUN/TURN/Cloudflare/Coturn ICE helpers, shared `--ice-server` CLI args
libs/libwish         # WHIP/WHEP client utilities
libs/net4mqtt        # TCP/UDP-over-MQTT proxy / tunnel
libs/playwright-whep # Rust-callable Playwright WHEP test harness
libs/rtsp            # RTSP client/server helpers
libs/signal          # OS signal handling
libs/storage         # OpenDAL-backed storage abstraction
libs/version         # build-time version info (shadow-rs)
liveion              # core SFU library
liveman              # cluster manager / controller
livetwo              # WHIP/WHEP <-> RTP/RTSP conversion library
livehal              # native capture/encoder backend (C++ pipeline)
```

### Binaries Produced

Built from `src/bin/` or `src/<name>.rs` in the root crate:

- `live777`      — main SFU server (uses `liveion`).
- `liveman`      — cluster manager for multiple `live777` nodes.
- `livetwo`      — provided as a library; command tools below use it.
- `whipinto`     — push RTP/RTSP into a WHIP endpoint; with the `rsmpeg`
  feature it also accepts a `synth://<vcodec>?...` input that publishes
  in-process generated test frames (no external encoder needed).
- `whepfrom`     — pull a WHEP stream and output RTP/RTSP.
- `whepwright`   — browser-based WHEP playback tester (feature gated).
- `net4mqtt`     — net-over-MQTT proxy binary.
- `livenil`      — cluster nil/bare runner for local multi-node tests.
- `datachannel_loadtest` — load-test binary (feature gated).
- `livewrk`       — load-testing tool (named after `wrk`) with `whip`
  (requires `rsmpeg`), `whep` subcommands.

### WebUI Packages (`web/*`)

- `player-core`  — WHEP player core: framework-agnostic playback engine
  (`WhepPlaybackCore` in `whep-core.ts`) plus WebRTC stats helpers.
- `player-vue`   — Vue WHEP player component library
  (`@binbat/whep-player-vue`): `useWhepPlayback` composable, `WhepPlayer`,
  `PlayerSurface`, `StatsForNerds`, `StandaloneWhepPlayer` (full-page,
  query-param driven; serves `/tools/player.html` in both admin apps).
- `debugger`     — debugging UI widget.
- `shared`       — shared Vue components/composables for the two admin apps
  (streams table, dialogs, SSE/refresh composables; consumed via the `@`
  alias, no package.json).
- `liveion`      — WebUI embedded by the `live777` binary.
- `liveman`      — WebUI embedded by the `liveman` binary.

Built assets are placed under `assets/<crate>/` and embedded at compile time via
`rust_embed::RustEmbed` when the `webui` feature is enabled.

## Build System

### Prerequisites

- Rust toolchain (stable; targets vary by platform).
- `pnpm` 10.20.0 or compatible.
- Node.js (CI uses `latest`).
- For WebUI builds: `pnpm install`.
- For native source features on Linux: `libcamera-dev`, `libv4l-dev`.
- For GStreamer-based tests: `gstreamer`, `gstreamer-rtsp-server`.
- For cross-compilation: `cross` from <https://github.com/cross-rs/cross>.

### Common Commands

```bash
# Install web dependencies
pnpm install

# Build the web UIs
pnpm -r build

# Build all Rust targets with all features (Linux; needs native deps)
cargo build --release --all-targets --all-features

# Run the main server with the embedded WebUI
cargo run --features=webui

# Run a local multi-node cluster
just run-cluster

# Build everything (web + Rust release)
just build

# Run the server with default config
cargo run --features=webui
```

### Feature Flags (Root Crate)

Key feature groups defined in the root `Cargo.toml`:

- `webui`          — embed static WebUI assets.
- `cascade`        — cluster cascading via `libwish`.
- `net4mqtt`       — enable MQTT-based tunneling.
- `recorder`       — stream recording to storage (FS/S3).
- `source`         — auto-start configured media sources.
- `source-sdp`     — SDP-file sources.
- `source-rtsp`    — RTSP sources.
- `source-whep`    — WHEP pull sources (static cascade-pull, built on livetwo).
- `source-all`     — enables all source types.
- `target-whip`    — WHIP push targets (static cascade-push).
- `target-rtp`     — RTP/UDP output targets: send a stream out as plain RTP
  to a multicast group or unicast address (the sender counterpart of the
  SDP-file source; multicast socket builders live in
  `livetwo::transport::multicast`).
- `target-rtsp`    — RTSP client-push output targets: push a stream to an
  RTSP server with ANNOUNCE/SETUP/RECORD (the counterpart of `source-rtsp`;
  built on the `libs/rtsp` client shared with whepfrom).
- `native-source`  — required base for capture/encoder features.
- `capture-libcamera`, `capture-v4l2` — video capture backends.
- `encoder-v4l2-m2m`, `encoder-rdk`, `encoder-rkmpp` — encoder backends.
- Platform presets: `native-rpi`, `native-generic-v4l2`, `native-rdk`,
  `native-rkmpp`.
- `whepwright`     — Playwright-based browser WHEP test harness.

Native capture/encoder features require Linux. On macOS/Windows CI the project
builds with `source-all,webui,net4mqtt,recorder,cascade,whepwright,target-whip,target-rtp,target-rtsp`
instead of `--all-features`.

### Cross-Compilation

`Cross.toml` configures `cross` images for `aarch64-unknown-linux-gnu` and
`armv7-unknown-linux-gnueabihf`. Per-platform cross images are published to
ghcr with the sysroot baked in; `*_SYSROOT` env vars override them with a
device-pulled sysroot when developing locally:

- Rockchip RKMPP (RK3588, RV1126B): `ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest`
  (sysroot at `/opt/rkmpp-sysroot`), or set `RKMPP_SYSROOT`.
- Raspberry Pi: `ghcr.io/binbat/crossbuilder-aarch64-rpi:latest`, or set
  `RPI_SYSROOT`.
- RDK X5: set `RDK_SYSROOT` (no published image; see the licensing note in
  `AGENTS.md` "Security Considerations").

Example:

```bash
CROSS_TARGET_AARCH64_UNKNOWN_LINUX_GNU_IMAGE=ghcr.io/binbat/crossbuilder-aarch64-rpi:latest \
  cross build --target aarch64-unknown-linux-gnu \
  --bin live777 --release \
  --no-default-features --features native-rpi,webui

# Rockchip RKMPP (RK3588, RV1126B), using the published cross image
CROSS_TARGET_AARCH64_UNKNOWN_LINUX_GNU_IMAGE=ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest \
  cross build --target aarch64-unknown-linux-gnu \
  --bin live777 --release \
  --no-default-features --features native-rkmpp,webui
```

`livehal/build.rs` reads `RPI_SYSROOT`/`RDK_SYSROOT`/`RKMPP_SYSROOT` to
configure `pkg-config` and linker paths.

## Runtime Architecture

- `live777` (`liveion`) is the edge SFU. It exposes WHIP publish endpoints,
  WHEP subscribe endpoints, admin/session APIs, Prometheus metrics, and an
  optional embedded WebUI.
- `liveman` sits in front of multiple `live777` nodes, proxies requests,
  manages cascade state, records via cluster policy, and stores recording
  indexes in a database.
- `livetwo` is the protocol-conversion engine used by `whipinto`/`whepfrom`
  and the `cascade` feature. `livetwo/src/whip/core.rs` is the single WHIP
  publish core (peer construction, connection waits, ICE diagnostics) shared
  by the RTP/RTSP bridge and the synthetic `whipsynth` publisher.
- `net4mqtt` exposes a local SOCKS proxy and tunnels traffic over MQTT for
  NAT/remote agents.

Configuration files:

- `conf/live777.toml` / `live777.toml` — main SFU config.
- `conf/liveman.toml` — cluster manager config.
- `conf/livenil/` — cluster nil config samples.

Important config sections: `http`, `stream`, `webrtc`, `ice_servers`, `auth`,
`recorder.storage`, `strategy`, `net4mqtt`.

There are no default ICE servers anywhere: liveion's `ice_servers` and
liveman's `extra_ice` are empty unless configured, so an out-of-the-box
deployment is LAN-only (host candidates only, no STUN/TURN advertised to
clients via Link headers).

## Code Organization Conventions

- Rust crate source lives in `src/` or `<crate>/src/`.
- `liveion/src/route/` — Axum route handlers (whip, whep, session, admin,
  stream, source, recorder, info, sdp).
- `liveion/src/forward/` — SFU forwarding core (publish, subscribe, channel,
  track, bridge, media, RTCP). Media statistics (issue #252): per-track and
  per-session counters live in `forward/stats.rs` (`MediaStats`); hot paths
  only `inc()` them (publish read loop in `track.rs`, subscriber write loop
  in `subscribe.rs`), and the manager's `stats_tick` (2 s) `sample()`s them
  into bitrates plus monotonic stream totals. Removal paths
  (`do_remove_publish_cleanup`, `do_remove_subscribe_cleanup`,
  `remove_virtual_tracks`) fold each departing flow's un-sampled tail into
  the totals, so counters stay exact across churn; `info()` adds each live
  flow's `unsampled()` tail to the folded totals so stream-level counters
  line up with the per-session ones between ticks; removal also refreshes
  the aggregate bitrate immediately so closed directions do not keep a stale
  rate until the next tick. Stats surface as the `stats` field on the
  stream/session API types and as the
  `live777_rtp_bytes_total{direction="in|out"}` Prometheus counter. RTCP is
  counted separately as
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
  drops vec children on its own, so without the removal deleted streams
  would stay in /metrics forever (registry growth on churn, and Prometheus
  would never mark them stale). The `stream` label is still unbounded on
  auto-create deployments while the streams are *live*; the removal keeps
  the registry bounded to live streams. The
  stream API includes `statsScope`: `node` for liveion snapshots and
  `clusterNodeWork` for liveman's merged sum of per-node work, where cascade
  hops are counted on each relay node.
  Snapshot freshness is cadence-driven: `stats_tick` unconditionally bumps
  a `watch` version that both SSE and the net4mqtt xdata notifier
  subscribe to, and both dedup on the exact serialized payload (which
  covers stats) — live rates push every tick while media flows, a silent
  stream's zero rate flushes exactly once, and an idle server sends
  nothing.
  DataChannel lifecycle: every channel gets a read and a write task
  (`dc_read_loop`/`dc_write_loop` in `forward/internal.rs`) bridging it to
  the per-stream publish/subscribe broadcast buses. The webrtc driver never
  delivers `OnClose` for a server-side `PeerConnection::close` (it aborts
  first), so each session carries a `CancellationToken` (`dc_cancel` on
  `PublishRTCPeerConnection`/`SubscribeRTCPeerConnection`, cancelled from
  `Drop`) that both loops `select!` on — without it the read loop parks on
  `poll()` forever, and its bus sender clone keeps the bus open, which used
  to pin the `[stream.x.channel]` UDP bridge (and its listen port) of a
  dead stream forever. `on_data_channel` only wires a channel whose peer
  still owns a live session (a late announce from a replaced/torn-down
  session is dropped), and both sides gate sends on `Connected`
  (`wait_for_peer_connected`, 15 s). The UDP bridge itself
  (`forward/channel.rs`) is owned by the forward: `PeerForwardInternal::close`
  cancels it and awaits the socket release, so a provisioned-stream reset
  rebinds the same port deterministically (`bind_with_retry` covers slow
  external holders). The client side mirrors this: whepfrom's channel
  bridge runs on a session-scoped token that `whep::from` cancels and
  awaits before returning.
- `liveion/src/stream/` — stream manager + source adapters. Every
  `[stream.<name>]` config entry is *provisioned*: pre-registered at startup
  (`Manager::provision_streams`), always listed in the API/Dashboard, exempt
  from orphan/auto-delete reapers, and rejected (409) on admin API
  create/delete. Internal teardowns (`Manager::teardown_stream`, used by RTSP
  re-ANNOUNCE and session cascades) reset a provisioned stream to standby
  with a `StreamDeleted`+`StreamCreated` pair instead of removing it. With
  `on_demand = true` the stream's sources start on the first subscriber
  (WHEP/cascade push/RTSP pull) and stop `on_demand_close_after_ms` after the
  last one leaves; source start/stop emits `PublishStarted`/`PublishStopped`
  with the synthesized `virtual-source` session id. On-demand readiness is
  judged by the source *bridge* (`SourceManager::has_bridge`), not source
  existence, and starts/stops serialize on a per-stream lock
  (`on_demand_locks`). The idle check (`on_demand_stream_idle`) counts real
  subscriber sessions (WHEP/cascade) and registered *virtual subscribers*
  (`Manager::virtual_subscribers`): internal consumers that tap the forward
  track broadcast directly instead of holding a subscribe session — static
  targets and RTSP pull clients. This is the subscribe-side counterpart of
  the `virtual-source` publisher: attach/detach
  (`add_virtual_subscriber`/`remove_virtual_subscriber`) emits
  `SubscribeStarted`/`SubscribeStopped`, `Manager::info`/`do_snapshot`
  synthesize always-Connected sessions with reserved `virtual-…` ids
  (e.g. `virtual-target-rtp-230.1.1.2:1720`, so dashboards show why an
  on-demand stream is running), the per-stream
  `live777_stream_sessions{kind="subscribe"}` gauge and the global
  `live777_subscribe` gauge include them, and the session-delete API
  rejects their ids (400 — they are owned by their consumer's supervisor).
  Without the registration the idle check stopped an on-demand stream's
  sources every `on_demand_close_after_ms` underneath its targets
  (live777#481). The recorder is intentionally not registered: it must not
  keep an on-demand source alive by itself. A WHIP publish onto a stream with an active source
  bridge is rejected (409) to avoid mixing two publishers' tracks. A second
  WHIP publish on an already-published stream instead *displaces* the
  incumbent (mediamtx-style override): the old session is torn down with
  `PublishStopped`/`SessionStopReason::Replaced`, and same-codec takeovers
  are seamless to subscribers via the media-generation machinery. Streams
  opt out with `strategy.override_publisher = false` (global or per-stream),
  restoring the 409 — except a plain-WHIP incumbent that is already
  `Disconnected`/`Failed`/`Closed` is displaced even then, so a fast
  reconnect does not bounce off the zombie session with a 409 while the
  disconnected-watchdog is still reaping it. Cascade-pull publishers are
  never displaced and always conflict (409), since their supervisor would
  reconnect and fight.
  Source encoder bitrate control (issue #409): adaptive bitrate is on by
  default for native encoder sources (`adaptive = false` opts out,
  `adaptive = { min_bitrate = … }` sets a custom floor; the default floor
  is the lowest tier's bitrate when tiers are declared, else
  `max(bitrate / 8, 300k)`).  The AIMD controller
  (`stream/source/adaptive_bitrate.rs`) retunes the running encoder from
  WHEP subscriber RTCP feedback (sampled by
  `forward/subscribe_quality.rs`); runtime retuning needs encoder-backend
  support (`EncoderBackend::setBitrate` in livehal — rkmpp and v4l2-m2m
  today). Two admin surfaces sit on top, deliberately separate:
  `GET /api/sources/{stream}/bitrate` is read-only encoder telemetry
  (drive mode `adaptive`/`fixed` plus the current rate), while
  `GET`/`POST /api/sources/{stream}/tier` is the control surface — a
  source-level ladder (`stream/source/tier.rs`): bitrate-only tiers
  retune in place, and a tier carrying `width`/`height`/`fps` switches
  the whole rung by rebuilding the capture+encoder pipeline
  (`NativeEncodedSource::reconfigure` — the RTP broadcast channels
  survive, so subscribers stay attached across the sub-second gap; a
  failed rebuild rolls back to the previous params). A per-stream
  `BitrateControl` (created for every native source in `create_bridge`)
  coordinates tier applies with the controller: the tier's bitrate
  becomes the AIMD's rung ceiling and resume seed without suspending
  it, and an external-change generation counter guarantees the
  controller re-seeds even for applies shorter than its 1 s tick.
- `liveion/src/event.rs` — typed stream-lifecycle events (`stream_created` …
  `subscribe_stopped` with reasons) on a single manager-wide broadcast bus.
  Consumers must tolerate `broadcast::RecvError::Lagged` by continuing the
  loop (and re-snapshotting where applicable).
- `liveion/src/recorder/` — recording pipeline (fmp4, segmenter, uploader,
  codec-specific writers).
- `liveion/src/hook.rs` — stream-lifecycle hook scripts (`[hooks]` global +
  `[stream.<name>.hooks]` per stream) run by a single FIFO executor:
  dispatcher forwards `StreamCreated`/`StreamDeleted`/`PublishStarted`/
  `PublishStopped` into an internal queue, then scripts run sequentially
  (global first, per-stream after, configured order) with per-script timeout
  and `on_error` policy.  One hook deliberately bypasses the queue:
  `on_source_changed` scripts run **synchronously inside** a source's
  parameter-set change (the admin tier API, `apply_source_tier`) before
  the capture+encoder re-provisioning — for hardware that must be
  reconfigured ahead of the pipeline rebuild (e.g. camera sensor mode
  gears on Rockchip-style V4L2 pipelines, where framerate is a
  sensor-mode property).  A hook failure aborts the apply when
  `on_error = "stop"` (`ApplyTierOutcome::HookFailed` → 500).  When the
  apply does not reach the target state (aborted, or the rebuild failed
  and rolled back), the scripts run again with the source's *current*
  state (`StreamSource::active_tier_state`, best effort) so hardware
  they switched is switched back.  Scripts see argv `<stream> <tier>`
  and `LIVE777_SOURCE_TIER` plus the source geometry/bitrate
  (`LIVE777_SOURCE_WIDTH`/`_HEIGHT`/`_FPS`/`_BITRATE`) — declared tier
  values on the pre-apply run, the pipeline's actual current values on
  the compensation run.
- `liveion/src/target.rs` — static WHIP push targets
  (`[[stream.<name>.targets]]`, declarative cascade-push; `target-whip`
  feature). One supervisor task per target keeps the push media-driven:
  established on `PublishStarted`, torn down on `PublishStopped` (the push
  negotiates per media epoch, so its codecs always match the current
  publisher), retried with source-style backoff (5 s doubling, 60 s cap),
  reconciled against the manager on event-bus lag; a target on an
  `on_demand` stream is standing demand: the supervisor registers as a
  virtual subscriber (`Manager::add_virtual_subscriber`, id
  `virtual-target-…`) for its whole lifetime so the sources are never
  idle-stopped underneath it, and (re)starts them whenever the stream has
  neither a publisher nor a push session, paced by the same backoff.
  `target::init` dispatches per target by URL scheme, so `whip://`,
  `rtp://` and `rtsp://` targets mix freely on one stream.
- `liveion/src/target_rtp.rs` — static RTP/UDP output targets
  (`rtp://group-or-host:port`; `target-rtp` feature), the sender
  counterpart of the SDP-file source: live777 acts as the multicast sender,
  keeping Unitree-style video-link receivers working unmodified. The
  supervisor mirrors the WHIP one (media-driven, same backoff/reconcile/
  standing-demand semantics) plus per-track send tasks that tap the forward
  track broadcast directly (the recorder / RTSP-server pattern, no
  subscribe session): first video track to the URL port, first audio track
  to port + 2 (RTP/AVP convention). Like the RTSP target, the epoch waits
  for the *negotiated* publish track counts
  (`PeerForwardInternal::negotiated_publish_track_counts`) instead of the
  first non-empty snapshot — tracks arrive one `on_track` each after
  `PublishStarted`, so a snapshot would race an AV publisher into a
  video-only send and SDP. Send tasks report their exit tagged
  with the epoch's generation, and the supervisor ignores tags that are not
  the live epoch — a belated exit from an already torn-down epoch must
  never cancel the fresh one, or every teardown self-sustains a restart
  storm. Video is re-packetized through livetwo's `RePayloadCodec`
  (SPS/PPS inlined ahead of every IDR, seeded from the codec fmtp via
  `with_sprop_params`), a PLI nudges the publisher on attach, and the video
  payload type can be pinned (`payload_type`, e.g. 96 for Unitree
  receivers); `sdp_file` writes a receiver-side SDP on send start that
  live777's own SDP-file source can ingest (live777 → live777 multicast
  cascade). Multicast sender/receiver socket builders, interface
  resolution (v4 address vs v6 index/name via `if_nametoindex`) and the
  dual-stack RTCP socket are shared in `livetwo::transport::multicast`
  (`multicast` feature), used by both `target-rtp` and `source-sdp`.
- `liveion/src/target_rtsp.rs` — static RTSP client-push output targets
  (`rtsp://[user:pass@]host:port/path[?transport=tcp|udp]`; `target-rtsp`
  feature), the counterpart of the RTSP source: live777 ANNOUNCE/SETUP/
  RECORDs the stream to an RTSP server (mediamtx, another live777's RTSP
  server, gst-rtsp-server) with the `libs/rtsp` client whepfrom also uses.
  The supervisor mirrors the RTP one (media-driven, same backoff/reconcile/
  standing-demand semantics, generation-tagged epochs) and the media plane
  is the same track-broadcast tap + `RePayloadCodec` + payload-type
  re-stamp; the ANNOUNCE SDP is built from the forward's track codecs in
  the shape of the RTSP server's DESCRIBE (`a=control:` per media, which
  the client session derives the SETUP URLs from). `PublishStarted` fires
  at negotiation time while tracks arrive one `on_track` each, so the
  epoch waits for the *negotiated* track counts
  (`PeerForwardInternal::negotiated_publish_track_counts`) instead of the
  first non-empty snapshot — an AV publisher whose audio track lands first
  would otherwise announce audio-only. UDP is the default transport
  (`?transport=tcp` selects interleaved, mirroring whepfrom); UDP senders
  bind the SETUP-announced local port because strict servers (mediamtx)
  drop RTP from any other source port. Teardown cancels the epoch token
  threaded into `rtsp::setup_rtsp_session`: TCP closes the connection,
  UDP sends TEARDOWN from the keep-alive loop, so the server releases the
  published path immediately and a re-push cannot collide with a zombie
  publisher. Server-side keyframe requests (a puller's PLI/FIR relayed by
  mediamtx or another live777) arrive on the interleaved RTCP channel /
  UDP RTCP port and are forwarded to the publisher via
  `send_rtcp_to_publish`.
- `liveman/src/route/` — proxy/cascade/admin routes. The dashboard's
  streams table is SSE-pushed like liveion's: `GET /api/sse/streams`
  (`route/stream.rs::sse`, same `?nodes=` filter as `GET /api/streams/`)
  sends the merged cluster view on connect and re-sends it whenever
  `Storage`'s `watch` change-version bumps (snapshot applies, eager
  stream-index writes) and on a 3 s idle cadence that doubles as the
  driver for the throttled lazy poll of poll-mode nodes. Identical
  consecutive payloads are suppressed, and the loop selects on the
  server's `CancellationToken` so a streaming response cannot hold
  graceful shutdown open.
- `liveman/src/service/` — business logic (database, recordings index).
- `liveman/src/entity/` + `migration/` — Sea-ORM entities and migrations.
- `libs/api/src/` — shared REST/WebRTC API types (`request`, `response`,
  `webrtc`, `recorder`, `path`, `strategy`).

## Development Conventions

- Follow `.editorconfig`: LF, UTF-8, trim trailing whitespace, final newline,
  4-space indent (2 for JSON), max line length 120.
- Rust code is formatted with `cargo fmt` and linted with `cargo clippy -D
  warnings`.
- Web code is formatted/linted with Biome (`biome.json`) and ESLint +
  TypeScript (`eslint.config.js`, `pnpm run lint`, `pnpm run typecheck`).
- Keep changes scoped to the modules the request implies; avoid unrelated
  refactors.
- Match surrounding style, naming, and comment density.
- Do not add new dependencies without confirming they are needed and
  compatible with the workspace versions.
- Do not commit secrets; config files in `conf/` are templates/examples.

## Testing

The project uses `cargo nextest` with configuration in `.config/nextest.toml`.

- Default profile: retries up to 4 times with exponential backoff.
- `ci` profile: 1 retry, 120 s slow-timeout, `fail-fast = false`.
- Integration tests that use FFmpeg, sockets, or browsers are forced serial
  (`serial-integration` test group) to avoid port/resource collisions.

Run tests:

```bash
# full workspace with coverage, matching the CI feature set
cargo llvm-cov nextest --profile ci --workspace \
  --features source-all,webui,net4mqtt,recorder,cascade,rsmpeg,whepwright,rtsp,target-whip,target-rtp,target-rtsp \
  --lcov --output-path lcov.info

# without coverage
cargo nextest run --workspace
```

Integration test binaries live in `tests/`:

- `tests/matrix/` — the end-to-end source × media-profile × player matrix
  harness (test binary `matrix`). Codec combinations are declared once in
  `tests/matrix/profile.rs`; sources live in `tests/matrix/source/`, players
  (livetwo+ffprobe, rsmpeg, Playwright) in `tests/matrix/player/`, and the
  shared liveion/port/wait/ffprobe infrastructure in
  `tests/matrix/runner.rs` and `tests/matrix/probe.rs`. The liveion RTSP
  server push→pull round-trip (former `tests/rtsp.rs`) and the full
  RTSP→WHIP→WHEP→RTSP conversion cycle (former `tests/rtsp2.rs`) live here
  as the `rtsp_roundtrip_*` and `rtsp_cycle_*` matrices.
- `tests/channel.rs`
- `tests/tests.rs` — liveion API smoke tests
- `tests/recorder.rs`
- `tests/livewrk_e2e.rs` — livewrk CLI end-to-end: real `livewrk` whip/whep
  subprocesses against in-process liveion, including the rotating decode
  verification (needs the `rsmpeg` feature)

Tests that create local WebRTC peers set
`LIVE777_WEBRTC_ICE_UDP_ADDRS=127.0.0.1:0` to force loopback ICE candidates in
CI.

Playwright browser tests need:

```bash
pnpm exec playwright install --with-deps chromium
export PLAYWRIGHT_BROWSERS_PATH=$PWD/.playwright
```

mediamtx interop tests (`whep_mediamtx_pull_*`, `rtsp_push_mediamtx_*` and
`rtsp_target_mediamtx_*` in
the matrix binary, live777#212) need a mediamtx binary: `just mediamtx`
downloads the pinned release into `target/`, or install mediamtx into `PATH`;
`MEDIAMTX_BIN` overrides the lookup. The tests skip when no binary is found.
They also run on Windows hosts, but skip on Windows CI: GitHub-hosted
Windows runners encode video at ~0.03x realtime, so media-heavy cases time
out downstream (the same flake class as a390dc7). The WHEP-source relay
matrix (`whep_source_livetwo_*`, two liveion instances per case) skips on
Windows CI for the same reason; the shared `runner::windows_ci()` helper
carries the check.

## Security Considerations

- WHIP/WHEP endpoints require a `Bearer` token unless `auth.tokens` is empty.
- `libs/auth` supports static tokens and HMAC-signed JWTs.
- `liveman` admin dashboard uses account-based auth (accounts configured in
  `liveman.toml`).
- ICE/TURN credentials can be configured statically or generated for Coturn
  (`--use-auth-secret`) and Cloudflare TURN via `libs/iceserver`.
- Recording storage supports local filesystem and S3/S3-compatible backends via
  OpenDAL; credentials belong in config files or environment, never in source.
- `liveman` database URL can be set via `DATABASE_URL`; default is SQLite
  (`sqlite://./liveman.db?mode=rwc`).

## Deployment & Packaging

- **Docker**: multi-stage Dockerfiles in `docker/` for `live777-server`
  (live777 + liveman), `live777-client` (self-contained FFmpeg + whipinto +
  whepfrom; its FFmpeg build mirrors `Dockerfile.ffmpeg`), `liveion`
  (live777 only), `liveman` (liveman only), `whipinto`, `whepfrom`,
  `net4mqtt`, `ffmpeg`, and `gstreamer` variants. Images are published to
  `ghcr.io/binbat/<app>`.
- **systemd**: service units in `conf/live777.service` and
  `conf/liveman.service`.
- **Monitoring**: `compose.monitoring.yml` brings up a Prometheus + Grafana
  stack scraping the live777 `/metrics` endpoint (always compiled in);
  scrape config in `conf/monitoring/prometheus.yml`, provisioned datasource
  and the `live777 overview` dashboard under `conf/monitoring/grafana/`.
  Just recipes: `just monitoring-up` / `just monitoring-down`. liveman has
  no Prometheus endpoint — scrape the live777 nodes directly.
- **Packages**: nFPM configs in `nfpm/` build `.deb`, `.rpm`, and Arch Linux
  packages; GitHub Actions upload them to releases.
- **Size-optimized builds are opt-in**: official release binaries use the
  default `release` profile. For size-sensitive deployments (embedded,
  containers) use the `release-size` profile (`Cargo.toml`: fat LTO, 1
  codegen unit, stripped, `panic=abort`, opt-level stays 3) plus
  `upx --best --lzma` — via `just build-size` / `just pack-size` locally,
  or `just cross-build-size <target> <features>` /
  `just cross-pack-size <target> <features>` for cross-compiled embedded
  targets (cross-rs; UPX packs foreign-arch ELFs from the host).
  Device-pinned shortcuts cover the supported embedded presets:
  `just rpi-pack-size` (uses the RPi cross image), `just rdk-pack-size`
  (needs `RDK_SYSROOT`), and `just v4l2-pack-size [target]`
  (armv7 by default).
- **Releases**: `.github/workflows/release.yml` builds for many targets
  including x86_64, aarch64, armv7, i686, riscv64, Android, Windows, and macOS.
- **Docs**: VitePress site in `docs/`; run `pnpm run docs:dev` / `docs:build`.
  `docs:build` also emits `llms.txt`, `llms-full.txt` and per-page `.md`
  (English only, the `zh` locale is excluded) via `vitepress-plugin-llms`.

## Useful Local Recipes (justfile)

```bash
just build            # web + Rust release build
just run              # cargo run --features=webui
just run-cluster      # local livenil cluster
just gst-whip-rtp-h264  # GStreamer WHIP ingest smoke test
just ffmpeg-rtp-h264    # FFmpeg WHIP ingest smoke test
just ffplay-rtp         # WHEP playback to ffplay via RTP
```

The `justfile` contains many grouped recipes for GStreamer, FFmpeg, RTSP, and
cycle tests; they are the fastest way to exercise a local `live777` instance.

## Quick Start for Agents

1. `pnpm install`
2. `cargo build --release --all-targets --features webui,source-all,recorder`
   (adjust features for your platform; native features need Linux).
3. `pnpm -r build` if you changed WebUI code.
4. Edit `live777.toml` or `conf/live777.toml` as needed.
5. `cargo run --features=webui` or `just run`.
6. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets --workspace --
   -D warnings`, and `cargo nextest run --workspace` before finishing.
