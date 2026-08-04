# Live777 — Agent Guide

This document is a concise orientation for AI coding agents working on the
Live777 repository. It is derived from the actual project files; if something
conflicts with the code, the code wins.

## Project Overview

Live777 is a lightweight, high-performance WebRTC SFU (Selective Forwarding
Unit) that uses the `WHIP`/`WHEP` protocols as its primary interface. It is
designed for real-time audio/video streaming and interoperates with clients
such as GStreamer, FFmpeg, OBS Studio, VLC, and browsers.

The repository is a mixed Rust + TypeScript project. Rust provides the media
server, protocol conversion, and command-line tools. TypeScript/Vite provides
the embedded WebUIs (Preact for the admin UIs, SolidJS for the player widgets).

- Repository: <https://github.com/binbat/live777>
- License: `MPL-2.0`
- Authors: BinBat Ltd <hey@binbat.com>
- Contributors must sign the CLA in `.github/CLA.md` before submitting work.

## Technology Stack

- **Rust** — edition 2024, workspace version `0.9.0`.
- **Async runtime** — Tokio.
- **HTTP/API layer** — Axum 0.8, `tower-http` (CORS, tracing).
- **WebRTC stack** — `webrtc`/`rtc-*` crates (`0.20.0-rc.2`), pinned to
  upstream `https://github.com/webrtc-rs/rtc` at revision `de84c7c8` via
  `[patch.crates-io]` until the next tag is released.
- **Web UI** — Vite 7, Preact (`web/liveion`, `web/liveman`, `web/shared`),
  SolidJS (`web/player-core`, `web/alone-player`, `web/debugger`), Tailwind
  CSS, DaisyUI, TypeScript. Docs site uses VitePress (`docs/`).
- **Package manager** — pnpm 10.20.0 (workspace covers `web/*`, see
  `pnpm-workspace.yaml`).
- **Storage** — OpenDAL for object/FS storage; Sea-ORM + SQLite (or Postgres)
  in `liveman` for recording indexes.
- **Media testing** — FFmpeg and GStreamer pipelines (see `justfile`);
  `rsmpeg` (system FFmpeg bindings) for in-process encode/decode test tools.
- **Task runner / local recipes** — `just` (`justfile`).

## Workspace Layout

The root `Cargo.toml` defines a workspace with these members (`libs/auth` is
pulled in as a path dependency of `liveion`/`liveman`):

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

Built from `src/<name>.rs` or `src/bin/<name>.rs` in the root crate:

- `live777`      — main SFU server (uses `liveion`); default-run binary.
- `liveman`      — cluster manager for multiple `live777` nodes.
- `livenil`      — cluster nil/bare runner for local multi-node tests.
- `whipinto`     — push RTP/RTSP into a WHIP endpoint; with the `rsmpeg`
  feature it also accepts a `synth://<vcodec>?...` input that publishes
  in-process generated test frames (no external encoder needed).
- `whepfrom`     — pull a WHEP stream and output RTP/RTSP.
- `whepprobe`    — WHEP stream probe/inspector (requires `rsmpeg`).
- `whipsynth`    — synthetic WHIP publisher generating test frames in-process
  (requires `rsmpeg`).
- `whepwright`   — browser-based WHEP playback tester (requires `whepwright`).
- `net4mqtt`     — net-over-MQTT proxy binary.
- `datachannel_loadtest` — DataChannel load-test binary (requires `source`).
- `livewrk`      — load-testing tool (named after `wrk`) with `whip`
  (requires `rsmpeg`) and `whep` subcommands.

### WebUI Packages (`web/*`)

- `shared`       — Preact components/hooks/API client shared by the admin UIs.
- `liveion`      — WebUI embedded by the `live777` binary (Preact).
- `liveman`      — WebUI embedded by the `liveman` binary (Preact).
- `player-core`  — reusable WHEP player component (SolidJS library).
- `alone-player` — standalone player widget (SolidJS).
- `debugger`     — debugging UI widget (SolidJS).

Built assets are placed under `assets/<crate>/` and embedded at compile time via
`rust_embed::RustEmbed` when the `webui` feature is enabled.

## Build System

### Prerequisites

- Rust toolchain (stable; targets vary by platform).
- `pnpm` 10.20.0 or compatible, plus Node.js.
- For WebUI builds: `pnpm install`.
- For `rsmpeg` features: system FFmpeg development libraries.
- For native source features on Linux: `libcamera-dev`, `libv4l-dev`.
- For GStreamer-based tools/tests: `gstreamer`, `gstreamer-rtsp-server`
  (Debian: `libgstreamer1.0-dev libgstrtspserver-1.0-dev`).
- For cross-compilation: `cross` from <https://github.com/cross-rs/cross>.

### Common Commands

```bash
# Install web dependencies
pnpm install

# Build the web UIs
pnpm -r build          # or: pnpm run build:liveion / build:liveman

# Build all Rust targets with all features (Linux; needs native deps)
cargo build --release --all-targets --all-features

# Build everything (web + Rust release)
just build

# Run the main server with the embedded WebUI
cargo run --features=webui        # or: just run

# Run a local multi-node cluster
just run-cluster
```

### Feature Flags (Root Crate)

Key feature groups defined in the root `Cargo.toml`:

- `webui`          — embed static WebUI assets.
- `cascade`        — cluster cascading via `libwish`.
- `net4mqtt`       — enable MQTT-based tunneling.
- `recorder`       — stream recording to storage (FS/S3).
- `rtsp`           — built-in RTSP server in liveion (push via ANNOUNCE/RECORD,
  pull via DESCRIBE/PLAY).
- `source`         — auto-start configured media sources.
- `source-sdp`     — SDP-file sources.
- `source-rtsp`    — RTSP sources.
- `source-whep`    — WHEP pull sources (static cascade-pull, built on livetwo).
- `source-all`     — enables all source types.
- `target-whip`    — WHIP push targets (static cascade-push).
- `rsmpeg`         — enables rsmpeg-based tools (`whepprobe`, `whipsynth`) and
  livetwo rsmpeg support.
- `native-source`  — required base for capture/encoder features.
- `capture-libcamera`, `capture-v4l2` — video capture backends.
- `encoder-v4l2-m2m`, `encoder-rdk`, `encoder-rkmpp` — encoder backends.
- Platform presets: `native-rpi`, `native-generic-v4l2`, `native-rdk`,
  `native-rk3588`.
- `whepwright`     — Playwright-based browser WHEP test harness.

Native capture/encoder features require Linux. On macOS/Windows CI the project
builds with `source-all,webui,net4mqtt,recorder,cascade,rsmpeg,whepwright,rtsp,target-whip`
instead of `--all-features` (this is also the CI test feature set).

There is also a size-optimized `[profile.release-size]` (fat LTO, single
codegen unit, stripped symbols, `panic = "abort"`):

```bash
cargo build --profile release-size --bin live777 --features ...
```

### Cross-Compilation

`Cross.toml` configures `cross` images for `aarch64-unknown-linux-gnu` and
`armv7-unknown-linux-gnueabihf`. For Raspberry Pi libcamera builds you need a
sysroot and `PI_SYSROOT` set; for RDK X5 builds use `RDK_SYSROOT`; for
Rockchip RK3588 (RKMPP) builds use the RKMPP cross image
(`ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest`, MPP sysroot baked at
`/opt/rkmpp-sysroot`) or set `RK_MPP_SYSROOT` to override it with a sysroot
pulled from a device. Example:

```bash
export PI_SYSROOT=/path/to/pi-sysroot
cross build --target aarch64-unknown-linux-gnu \
  --bin live777 --release \
  --no-default-features --features native-rpi,webui

# Rockchip RK3588 (RKMPP), using the published cross image
CROSS_TARGET_AARCH64_UNKNOWN_LINUX_GNU_IMAGE=ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest \
  cross build --target aarch64-unknown-linux-gnu \
  --bin live777 --release \
  --no-default-features --features native-rk3588,webui
```

`livehal/build.rs` reads `PI_SYSROOT`/`RDK_SYSROOT`/`RK_MPP_SYSROOT` to
configure `pkg-config` and linker paths. The `*-gcc-wrapper.sh` scripts at
the repo root are linker wrappers used by these cross setups.

### Luckfox Pico (RV1106, uClibc)

The RV1106 build cross-compiles the standard `armv7-unknown-linux-gnueabihf`
(glibc) Rust target against the Luckfox uClibc toolchain and rootfs. It uses
the RKMPI/Rockit VENC backend (`encoder_rockit.cpp`); the standard MPP path
is not usable on this SoC (its venc540c adapter cannot create userspace
buffer groups or emit packets without rockit-managed memory, and
`RK_MPI_MMZ_Fd2Handle` rejects external dma-buf fds, so zero-copy input is
not possible either).

Machine-local prerequisites (not in git):

- Luckfox SDK toolchain `arm-rockchip830-linux-uclibcgnueabihf` (from the
  `luckfox-pico` SDK, under `tools/linux/toolchain/`).
- A sysroot (e.g. `~/luckfox-sysroot`) with `include/` (RKMPI + MPP
  headers) and `mpp-lib/` (`librockit.so`, `librockchip_mpp.so`,
  `librga.so`) matching the board's `/oem/usr/lib`.

Everything else is in the repo: `livehal/build.rs` auto-enables the uClibc
compat shims (`native-pipeline/uclibc_compat.c`, providing `getauxval` and
`posix_spawnattr_*`, compiled into the native static lib) whenever
`LUCKFOX_SYSROOT` is set for an arm target.

```bash
export LUCKFOX_SYSROOT=~/luckfox-sysroot
export LIVEHAL_RKMPP_ROCKIT=1   # selects the Rockit VENC backend on arm
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=/path/to/arm-rockchip830-linux-uclibcgnueabihf-gcc
cargo build --release --target armv7-unknown-linux-gnueabihf --bin live777 \
  --no-default-features --features native-rk3588,webui
```

Runtime notes on the board:

- Stop the stock `rkipc` app (`/oem/usr/bin/RkLunch-stop.sh`) before
  starting live777; it holds the camera. The ISP also needs an rkaiq
  process feeding params (`/oem/usr/bin/rkaiq_3A_server`), otherwise
  `/dev/video11` produces no frames ("waiting on params stream on event
  timeout").
- Set `webrtc.ice_udp_addrs` explicitly (e.g. `["172.32.0.93:0"]`): the
  `"auto"` discovery needs a default route, and on RNDIS-only devices it
  falls back to 127.0.0.1 candidates, which remote browsers cannot reach.
- RV1106 has ~100 MB CMA, half of it permanently consumed. Segfaulted
  processes leak kernel-side CMA; if allocation errors cascade
  (`cma_alloc ... ret: -12`), reboot.

## Runtime Architecture

- `live777` (`liveion`) is the edge SFU. It exposes WHIP publish endpoints,
  WHEP subscribe endpoints, admin/session APIs, Prometheus metrics, an
  optional built-in RTSP server (`rtsp` feature), and an optional embedded
  WebUI.
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

- The server takes `-c/--config <path>`; the default path is `live777.toml`
  in the current directory. If the file does not exist, built-in defaults are
  used with a warning.
- `conf/live777.toml` — main SFU config template.
- `conf/liveman.toml` — cluster manager config template.
- `conf/livenil/` — cluster nil config samples.

Important config sections: `http`, `stream`, `webrtc`, `ice_servers`, `auth`,
`recorder.storage`, `strategy`, `net4mqtt`.

## Code Organization Conventions

- Rust crate source lives in `src/` or `<crate>/src/`.
- `liveion/src/route/` — Axum route handlers (whip, whep, session, admin,
  stream, strategy, source, recorder, info, sdp).
- `liveion/src/forward/` — SFU forwarding core (publish, subscribe, channel,
  track, bridge, media, RTCP, codec compatibility, AV1
  assembler/repacketizer). Media statistics (issue #252): per-track and
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
  `live777_rtp_bytes_total{direction="in|out"}` Prometheus counter. The
  stream API includes `statsScope`: `node` for liveion snapshots and
  `clusterNodeWork` for liveman's merged sum of per-node work, where cascade
  hops are counted on each relay node.
  Snapshot freshness is cadence-driven: `stats_tick` unconditionally bumps
  a `watch` version that both SSE and the net4mqtt xdata notifier
  subscribe to, and both dedup on the exact serialized payload (which
  covers stats) — live rates push every tick while media flows, a silent
  stream's zero rate flushes exactly once, and an idle server sends
  nothing.
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
- `liveion/src/rtsp_server/` — the built-in RTSP server (`rtsp` feature):
  accept push (ANNOUNCE/RECORD) and serve pull (DESCRIBE/PLAY).
- `liveion/src/hook.rs` — stream-lifecycle hook scripts (`[hooks]` global +
  `[stream.<name>.hooks]` per stream) run by a single FIFO executor:
  dispatcher forwards `StreamCreated`/`StreamDeleted`/`PublishStarted`/
  `PublishStopped` into an internal queue, then scripts run sequentially
  (global first, per-stream after, configured order) with per-script timeout
  and `on_error` policy.
- `liveion/src/target.rs` — static WHIP push targets
  (`[[stream.<name>.targets]]`, declarative cascade-push; `target-whip`
  feature). One supervisor task per target keeps the push media-driven:
  established on `PublishStarted`, torn down on `PublishStopped` (the push
  negotiates per media epoch, so its codecs always match the current
  publisher), retried with source-style backoff (5 s doubling, 60 s cap),
  reconciled against the manager on event-bus lag; a target on an
  `on_demand` stream acts as standing demand: its sources are (re)started
  whenever the stream has neither a publisher nor a push session, paced by
  the same backoff.
- `liveion/src/metrics.rs` — Prometheus metric registration
  (`metrics_register()` is called first thing in `main`).
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
- Web code: Biome (`biome.json`, `pnpm run check`) covers only
  `web/alone-player`, `web/player-core`, `web/debugger`, and
  `libs/playwright-whep/static`; ESLint + TypeScript (`eslint.config.js`,
  `pnpm run lint`, `pnpm run typecheck`) cover the whole web tree.
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
- Two test groups control parallelism: `resource-limited` (max 4 threads) for
  unit/lib tests, and `serial-integration` (max 1 thread) for the integration
  binaries `channel`, `tests`, `matrix`, and `livewrk_e2e`, which exercise
  FFmpeg, local sockets, and/or real browsers. The `ci` profile additionally
  serializes the four `net4mqtt` UDP tests.

Run tests:

```bash
# full workspace with coverage, matching the CI feature set
cargo llvm-cov nextest --profile ci --workspace \
  --features source-all,webui,net4mqtt,recorder,cascade,rsmpeg,whepwright,rtsp,target-whip \
  --lcov --output-path lcov.info

# without coverage
cargo nextest run --workspace
```

Integration test binaries live in `tests/` (`common.rs` is a shared helper
module, not a test binary):

- `tests/matrix/` — the end-to-end source × media-profile × player matrix
  harness (test binary `matrix`). Codec combinations are declared once in
  `tests/matrix/profile.rs`; sources live in `tests/matrix/source/`, players
  (livetwo+ffprobe, rsmpeg, Playwright) in `tests/matrix/player/`, and the
  shared liveion/port/wait/ffprobe infrastructure in
  `tests/matrix/runner.rs` and `tests/matrix/probe.rs`. The liveion RTSP
  server push→pull round-trip and the full RTSP→WHIP→WHEP→RTSP conversion
  cycle live here as the `rtsp_roundtrip_*` and `rtsp_cycle_*` matrices.
- `tests/tests.rs` — liveion API smoke tests.
- `tests/channel.rs` — DataChannel tests.
- `tests/whipsynth_packets.rs` — synthetic-publisher packet tests.
- `tests/livewrk_e2e.rs` — livewrk CLI end-to-end: real `livewrk` whip/whep
  subprocesses against in-process liveion, including the rotating decode
  verification (needs the `rsmpeg` feature).

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
downloads the pinned release (version in `mediamtx.version`, shared with the
CI action) into `target/`, or install mediamtx into `PATH`; `MEDIAMTX_BIN`
overrides the lookup. The tests skip when no binary is found. They also run
on Windows hosts, but skip on Windows CI: GitHub-hosted Windows runners
encode video at ~0.03x realtime, so media-heavy cases time out downstream
(the same flake class as a390dc7). The WHEP-source relay matrix
(`whep_source_livetwo_*`, two liveion instances per case) skips on Windows CI
for the same reason; the shared `runner::windows_ci()` helper carries the
check.

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
  `whipinto`, `whepfrom`, `net4mqtt`, `ffmpeg`, and `gstreamer` variants;
  `compose.yml` at the repo root ties them together. Images are published to
  `ghcr.io/binbat/<app>`.
- **systemd**: service units in `conf/live777.service` and
  `conf/liveman.service`.
- **Packages**: nFPM configs in `nfpm/` build `.deb`, `.rpm`, and Arch Linux
  packages; GitHub Actions upload them to releases.
- **CI**: `.github/workflows/rust.yml` (build + nextest/llvm-cov),
  `quality.yml` (fmt/clippy/lint), `docker.yml`, and `release.yml` (builds for
  many targets including x86_64, aarch64, armv7, i686, riscv64, Android,
  Windows, and macOS).
- **Docs**: VitePress site in `docs/` (English under `docs/guide`, Chinese
  under `docs/zh`); run `pnpm run docs:dev` / `docs:build`.

## Useful Local Recipes (justfile)

```bash
just build              # web + Rust release build
just run                # cargo run --features=webui
just run-cluster        # local livenil cluster
just build-tools        # compile tools/test-rtsp-server.c (needs GStreamer dev libs)
just mediamtx           # download pinned mediamtx into target/ for interop tests
just gst-whip-rtp-h264  # GStreamer WHIP ingest smoke test
just ffmpeg-rtp-h264    # FFmpeg WHIP ingest smoke test
just ffplay-rtp         # WHEP playback to ffplay via RTP
just livewrk-whip 100 60  # WHIP publish load test (streams load-0..load-99)
just livewrk-whep 100 60  # WHEP subscribe load test
```

The `justfile` contains many grouped recipes for GStreamer, FFmpeg, RTSP
push/pull, cycle tests, and load tests; they are the fastest way to exercise
a local `live777` instance.

## Quick Start for Agents

1. `pnpm install`
2. `cargo build --release --all-targets --features webui,source-all,recorder`
   (adjust features for your platform; native features need Linux).
3. `pnpm -r build` if you changed WebUI code.
4. Edit `conf/live777.toml` and pass it with `-c`, or drop a `live777.toml`
   in the working directory (the default lookup path).
5. `cargo run --features=webui` or `just run`.
6. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets --workspace --
   -D warnings`, and `cargo nextest run --workspace` before finishing; for web
   changes also run `pnpm run check` and `pnpm run lint`.
