# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking Changes

- **`liveion` configured streams are now provisioned (always registered).** Every `[stream.<name>]` config entry is registered at startup and listed in `GET /api/streams` and the Dashboard even while idle, is exempt from the orphan reaper and `auto_delete_whip`/`auto_delete_whep` timeouts, and can no longer be created or deleted through the admin API — `POST` / `DELETE /api/streams/{stream}` on a provisioned stream now return `409 Conflict`. `StreamCreated` for these streams fires once at startup (so `[hooks]` scripts and `recorder.auto_streams` matching them trigger earlier than before); no `StreamDeleted` is ever emitted for them, except paired with an immediate `StreamCreated` when an internal teardown (RTSP re-ANNOUNCE, publisher-leave cascade) resets the stream to standby — that pair carries the new `reset` delete reason (`LIVE777_REASON=reset` for hooks) instead of `api-deleted`.
- **Removed webhook support from `liveion`.** The `[webhook]` configuration section and the `webhooks` list are no longer recognized. Existing configurations that still contain a `[webhook]` section will have that section silently ignored. Remove it before upgrading to keep configs tidy.
- **WHIP/WHEP session IDs are now UUIDs** (previously an opaque hash of an internal pointer). Session IDs remain opaque strings for API clients, but anything pattern-matching the old 32-hex format must be updated.
  - Webhook-style push notifications are replaced by Server-Sent Events (`GET /api/sse/streams`) and the `net4mqtt` xdata channel, both of which push full stream-state snapshots when the state changes.
  - The `/api/sse/events` endpoint has been removed. Use `/api/sse/streams` instead.

### Added

- `liveion` WHEP pull source (new `source-whep` feature, included in `source-all`): a configured stream source `url = "whep://[token@]host:port/whep/<stream>"` pulls media from an upstream WHEP endpoint — the declarative, static form of `cascade-pull` — and takes part in the full source lifecycle: `on_demand` start/stop, automatic reconnect, codec-readiness gating, and subscriber RTCP feedback (e.g. PLI keyframe requests) forwarded to the upstream publisher. livetwo exposes the shared `livetwo::whep::forward_rtcp_to_peer` helper used both here and by whepfrom's TCP output.
- `liveion` on-demand sources: `[stream.<name>] on_demand = true` keeps the stream's configured sources (camera/encoder, RTSP pull, SDP) stopped until the first subscriber (WHEP, cascade push, or RTSP pull) arrives, and stops them again `on_demand_close_after_ms` after the last one leaves. The first subscriber waits up to `on_demand_start_timeout_ms` for the source to become ready so its SDP answer already contains the tracks; subscribers arriving while a start is in flight wait for it instead of receiving a track-less answer, and if the source is not ready in time the subscribe request fails (and cleans up) instead of hanging forever. The Dashboard shows `standby`/`on-demand` badges for these streams and `config` for other provisioned streams; `api::response::Stream` gained matching `provisioned`/`onDemand` fields.
- `liveion` stream-lifecycle hooks: the new `[hooks]` section (global) and `[stream.<name>.hooks]` (per stream) run external scripts on stream lifecycle events — stream created/deleted (`on_stream_created`/`on_stream_deleted`) and publish started/stopped (`on_publish_started`/`on_publish_stopped`) — e.g. to power a capture device / hardware encoder on when media is actually wanted and off when the last consumer leaves. Publish hooks receive the publisher session id as `LIVE777_SESSION` (configured sources report `virtual-source`) and, for stop events, the reason (`peer-closed`/`api-deleted`/`idle-timeout`). Scripts receive `<event> <stream> [reason]` as argv and the same values as `LIVE777_EVENT`/`LIVE777_STREAM`/`LIVE777_REASON`; execution is a single FIFO queue (global hooks first, then per-stream, in configured order) with per-script `timeout_ms` and an `on_error = "stop"|"continue"` policy.

### Changed

- `liveion` now rejects a WHIP publish onto a stream whose configured source is actively feeding tracks (and refuses to start an on-demand source while a WHIP publisher is live) with `409 Conflict`, instead of mixing both publishers' tracks into every subscriber.
- `liveion` configured sources (always-on and on-demand alike) now emit `PublishStarted`/`PublishStopped` lifecycle events with the synthesized `virtual-source` session id when they start/stop, and `metrics::PUBLISH` counts them. Recording of on-demand streams matching `recorder.auto_streams` is driven by these publish events (record only while the source is active) instead of `StreamCreated`/`StreamDeleted`.
- `liveion` stream lifecycle events are now typed (`liveion::event::Event`) and travel on a single manager-wide broadcast bus: `StreamDeleted` emission is centralized into one funnel so it always pairs with its metrics update, and every consumer (SSE, net4mqtt, recorder) tolerates broadcast lag by re-syncing instead of silently stopping.
- `liveion` subscribe-side RTP write errors are now retried with a 3s bound while the peer may still be coming up, instead of being classified by matching `webrtc` crate error strings.
- `liveion` URL-source (RTSP/WHEP) reconnects now back off exponentially (5 s doubling, capped at 60 s) instead of retrying on a fixed 5 s interval, and the reconnect wait responds to source shutdown immediately instead of sleeping it out.
- `liveion` now exposes a single SSE endpoint `/api/sse/streams` that pushes a full snapshot of all streams whenever stream state changes.
- `net4mqtt` xdata messages now carry the sender identity as part of the channel tuple `(sender_id, key, payload)`. The receiver no longer needs to trust a user-supplied `alias` field inside the payload.
- `liveman` consumes `net4mqtt` xdata `streams` messages and uses the message metadata as the node alias.
- `liveman` static nodes now support a `mode` field (`"poll"` or `"sse"`, default `"poll"`). `poll` nodes continue to be updated by periodic HTTP polling, while `sse` nodes subscribe to the upstream `/api/sse/streams` endpoint for stream-state snapshots. `net4mqtt` nodes are unaffected and still update through `xdata`.
- `liveion` SSE `/api/sse/streams` and `net4mqtt` xdata `streams` now deduplicate snapshots: a new message is only emitted when the actual stream-state snapshot differs from the previous one.
- `liveion` `net4mqtt` xdata `streams` payload and vdata online payload no longer contain a redundant `alias` field; the sender identity is provided by the MQTT topic/channel metadata.
- `liveion` `net4mqtt` xdata channel is now bounded (capacity 64) with backpressure: outdated snapshots are dropped when the channel is full.
- `liveman` SSE subscriber now uses a 10s connect timeout, no per-request read timeout, and 30s TCP keepalive to recover from hung connections without frequent idle reconnects.
- `liveman` no longer polls `/api/strategy` for dynamically discovered `net4mqtt` nodes.

### Fixed

- Fixed `liveion` event consumers (recorder, net4mqtt notifier, SSE handler) silently exiting their receive loops on broadcast-channel lag bursts, which could permanently stop recorder auto start/stop and state notifications.
- Fixed WHEP subscribe session-registration errors being swallowed, which could return a successful answer to the client without a working session.
- `liveion` now logs a warning when a peer connection enters the `Disconnected` state (previously a lifecycle blind spot until it escalated to `Failed`), and closes the peer if it is still disconnected after 5s so sessions are torn down instead of lingering as zombies. Subscribe-side RTP forwarding now waits out a transient `Disconnected` state instead of permanently stopping on the first write error during it.
- Fixed `liveion` leaking the peer connection when WHIP/cascade publish or subscribe setup failed mid-handshake; the peer is now closed on every pre-registration failure path.
- Fixed a channel-sender leak in `net4mqtt` when `XDataConfig.receiver` was not provided.
- Changed MQTT subscribe/publish calls in `net4mqtt` to propagate errors instead of panicking on connection failures.
- Fixed `liveman` storage update logic so that stale stream/session mappings for a node are cleared before applying a new snapshot.
- Fixed task leaks in `liveion` net4mqtt reconnection logic.
- Fixed `GET /api/sources`, `GET /api/sources/{stream}` and `GET /api/sources/{stream}/state` dropping the request (handler task panic, empty reply) whenever a source existed: source state moved from `tokio::sync::RwLock::blocking_read` — which panics inside an async runtime — to a plain `std::sync::RwLock`.
- Fixed `liveion` failing to compile with only `source-sdp` (missing `rtsp_codec` module gate) or only `source` (missing `livetwo` dependency), and cleaned up the `dead_code`/unused warnings across feature combinations by narrowing `cfg` gates to the features that actually use each item; the unused `PeerForward::first_video_codec` helper was removed.
- Fixed `libwish` WHIP/WHEP HTTP requests having no timeout: a peer that accepts the connection but never responds could wedge `whepfrom`/`whipinto`, cascade setup, and a configured source's reconnect loop or `stop()` forever. Requests now use a 5 s connect / 10 s overall timeout.
- Source URLs with embedded credentials (`rtsp://user:pass@…`, `whep://token@…`) are now redacted in `liveion` log lines instead of being written out in full.
