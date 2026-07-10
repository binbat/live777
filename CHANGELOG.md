# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking Changes

- **Removed webhook support from `liveion`.** The `[webhook]` configuration section and the `webhooks` list are no longer recognized. If your configuration contains a `[webhook]` section, the server will fail to start. Remove it before upgrading.
  - Webhook-style push notifications are replaced by Server-Sent Events (`GET /api/sse/streams`) and the `net4mqtt` xdata channel, both of which push full stream-state snapshots when the state changes.
  - The `/api/sse/events` endpoint has been removed. Use `/api/sse/streams` instead.

### Changed

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

- Fixed a channel-sender leak in `net4mqtt` when `XDataConfig.receiver` was not provided.
- Changed MQTT subscribe/publish calls in `net4mqtt` to propagate errors instead of panicking on connection failures.
- Fixed `liveman` storage update logic so that stale stream/session mappings for a node are cleared before applying a new snapshot.
- Fixed task leaks in `liveion` net4mqtt reconnection logic.
