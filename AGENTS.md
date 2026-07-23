# Live777 — Agent Guide

This document is a concise orientation for AI coding agents working on the
Live777 repository. It is derived from the actual project files; if something
conflicts with the code, the code wins.

## Project Overview

Live777 is a lightweight, high-performance WebRTC SFU (Selective Forwarding
Unit) that uses the `WHIP`/`WHEP` protocols as its primary interface. It is
designed for real-time audio/video streaming and interoperates with clients
such as GStreamer, FFmpeg, OBS Studio, VLC, and browsers.

The repository is a mixed Rust + TypeScript/Preact/Solid project. Rust provides
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
- **WebRTC stack** — `webrtc`/`rtc-*` crates, pinned to upstream
  `https://github.com/webrtc-rs/rtc` at revision `de84c7c8` via
  `[patch.crates-io]` until the next tag is released.
- **Web UI** — Vite, Preact, SolidJS, Tailwind CSS, DaisyUI, TypeScript.
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
libs/iceserver       # STUN/TURN/Cloudflare/Coturn ICE helpers
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

- `player-core`  — reusable WHEP player component.
- `alone-player` — standalone player widget.
- `debugger`     — debugging UI widget.
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
- `source-all`     — enables all source types.
- `native-source`  — required base for capture/encoder features.
- `capture-libcamera`, `capture-v4l2` — video capture backends.
- `encoder-v4l2-m2m`, `encoder-rdk`   — encoder backends.
- Platform presets: `native-rpi`, `native-generic-v4l2`, `native-rdk`.
- `whepwright`     — Playwright-based browser WHEP test harness.

Native capture/encoder features require Linux. On macOS/Windows CI the project
builds with `source-all,webui,net4mqtt,recorder,cascade,whepwright` instead of
`--all-features`.

### Cross-Compilation

`Cross.toml` configures `cross` images for `aarch64-unknown-linux-gnu` and
`armv7-unknown-linux-gnueabihf`. For Raspberry Pi libcamera builds you need a
sysroot and `PI_SYSROOT` set; for RDK X5 builds use `RDK_SYSROOT`. Example:

```bash
export PI_SYSROOT=/path/to/pi-sysroot
cross build --target aarch64-unknown-linux-gnu \
  --bin live777 --release \
  --no-default-features --features native-rpi,webui
```

`livehal/build.rs` reads `PI_SYSROOT`/`RDK_SYSROOT` to configure `pkg-config`
and linker paths.

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

## Code Organization Conventions

- Rust crate source lives in `src/` or `<crate>/src/`.
- `liveion/src/route/` — Axum route handlers (whip, whep, session, admin,
  stream, strategy, source, recorder, info, sdp).
- `liveion/src/forward/` — SFU forwarding core (publish, subscribe, channel,
  track, bridge, media, RTCP).
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
  (`on_demand_locks`). A WHIP publish onto a stream with an active source
  bridge is rejected (409) to avoid mixing two publishers' tracks.
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
  and `on_error` policy.
- `liveman/src/route/` — proxy/cascade/admin routes.
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
  --features source-all,webui,net4mqtt,recorder,cascade,rsmpeg,whepwright,rtsp \
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

mediamtx interop tests (`whep_mediamtx_pull_*` and `rtsp_push_mediamtx_*` in
the matrix binary, live777#212) need a mediamtx binary: `just mediamtx`
downloads the pinned release into `target/`, or install mediamtx into `PATH`;
`MEDIAMTX_BIN` overrides the lookup. The tests skip when no binary is found.
They also run on Windows hosts, but skip on Windows CI: GitHub-hosted
Windows runners encode video at ~0.03x realtime, so media-heavy cases time
out downstream (the same flake class as a390dc7).

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

- **Docker**: multi-stage Dockerfiles in `docker/` for `live777`, `liveman`,
  `whipinto`, `whepfrom`, `net4mqtt`, `ffmpeg`, and `gstreamer` variants.
  Images are published to `ghcr.io/binbat/<app>`.
- **systemd**: service units in `conf/live777.service` and
  `conf/liveman.service`.
- **Packages**: nFPM configs in `nfpm/` build `.deb`, `.rpm`, and Arch Linux
  packages; GitHub Actions upload them to releases.
- **Releases**: `.github/workflows/release.yml` builds for many targets
  including x86_64, aarch64, armv7, i686, riscv64, Android, Windows, and macOS.
- **Docs**: VitePress site in `docs/`; run `pnpm run docs:dev` / `docs:build`.

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
