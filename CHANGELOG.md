# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking Changes

- **Removed webhook support from `liveion`.** The `[webhook]` configuration section and the `webhooks` list are no longer recognized. Existing configurations that still contain a `[webhook]` section will have that section silently ignored. Remove it before upgrading to keep configs tidy.
- **WHIP/WHEP session IDs are now UUIDs** (previously an opaque hash of an internal pointer). Session IDs remain opaque strings for API clients, but anything pattern-matching the old 32-hex format must be updated.
  - Webhook-style push notifications are replaced by Server-Sent Events (`GET /api/sse/streams`) and the `net4mqtt` xdata channel, both of which push full stream-state snapshots when the state changes.
  - The `/api/sse/events` endpoint has been removed. Use `/api/sse/streams` instead.

### Changed

- `liveion` stream lifecycle events are now typed (`liveion::event::Event`) and travel on a single manager-wide broadcast bus: `StreamDown` emission is centralized into one funnel so it always pairs with its metrics update, and every consumer (SSE, net4mqtt, recorder) tolerates broadcast lag by re-syncing instead of silently stopping.
- `liveion` subscribe-side RTP write errors are now retried with a 3s bound while the peer may still be coming up, instead of being classified by matching `webrtc` crate error strings.
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
