# Live777 — Agent Guide

This document is a concise orientation for AI coding agents working on the
Live777 repository. It is derived from the actual project files; if something
conflicts with the code, the code wins.

Scope policy: this root file carries only repo-wide conventions, commands and
pointers. Module behavior details live in the nearest `AGENTS.md` —
[liveion/AGENTS.md](liveion/AGENTS.md) (SFU core: forwarding, streams,
sources, targets, hooks), [livetwo/AGENTS.md](livetwo/AGENTS.md) (WHIP/WHEP
conversion), [livehal/AGENTS.md](livehal/AGENTS.md) (native backends,
cross-compilation). Check them when working in those subtrees, and update the
nearest file when you change documented behavior.

## Project Overview

Live777 is a low-latency WebRTC platform for audio, video and data: a
lightweight, high-performance SFU (Selective Forwarding Unit) core with a
two-way RTP/RTSP gateway, using the `WHIP`/`WHEP` protocols as its primary
interface. It interoperates with clients such as GStreamer, FFmpeg, OBS
Studio, VLC, and browsers.

The repository is a mixed Rust + TypeScript/Vue project. Rust provides
the media server, protocol conversion, and command-line tools. TypeScript/Vite
provides the embedded WebUIs.

- Repository: <https://github.com/binbat/live777>
- License: `MPL-2.0`
- Authors: BinBat Ltd <hey@binbat.com>
- Contributors must sign the CLA in `.github/CLA.md` before submitting work.

## Technology Stack

- **Rust** — edition 2024, workspace version `0.9.0`; Tokio async runtime.
- **HTTP/API layer** — Axum, `tower-http` (CORS, tracing).
- **WebRTC stack** — `webrtc`/`rtc-*` crates, release `0.21.1` from crates.io.
- **Web UI** — Vite, Vue 3, Tailwind CSS, DaisyUI, TypeScript.
- **Package manager** — pnpm 12.16.1 (workspace covers `web/*`).
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
libs/version         # build-time version info (shadow-rs; LIVE777_VERSION env
                     # overrides git-derived values, then Cargo.toml version)
liveion              # core SFU library (see liveion/AGENTS.md)
liveman              # cluster manager / controller
livetwo              # WHIP/WHEP <-> RTP/RTSP conversion library (see livetwo/AGENTS.md)
livehal              # native capture/encoder backends (see livehal/AGENTS.md)
```

### Binaries Produced

Built from `src/bin/` or `src/<name>.rs` in the root crate:

- `live777`      — main SFU server (uses `liveion`).
- `liveman`      — cluster manager for multiple `live777` nodes.
- `whipinto`     — push RTP/RTSP into a WHIP endpoint; with `rsmpeg` also
  accepts `synth://<vcodec>?...` in-process generated test frames.
- `whepfrom`     — pull a WHEP stream and output RTP/RTSP.
- `whipsynth`    — synthetic WHIP publisher: in-process test frames (`rsmpeg`).
- `whepprobe`    — WHEP probe: subscribe, decode for a bounded time, report
  human/JSON result (`rsmpeg`).
- `whepwright`   — browser-based WHEP playback tester (feature gated).
- `net4mqtt`     — net-over-MQTT proxy binary.
- `livenil`      — cluster nil/bare runner for local multi-node tests.
- `datachannel_loadtest` — load-test binary (feature gated).
- `livewrk`      — load-testing tool (named after `wrk`) with `whip`
  (`rsmpeg`) and `whep` subcommands.

### WebUI Packages (`web/*`)

- `player-core`  — framework-agnostic WHEP playback engine
  (`WhepPlaybackCore` in `whep-core.ts`) plus WebRTC stats helpers.
- `player-vue`   — Vue WHEP player component library
  (`@binbat/whep-player-vue`): `useWhepPlayback`, `WhepPlayer`,
  `PlayerSurface`, `StatsForNerds`, `StandaloneWhepPlayer` (serves
  `/tools/player.html` in both admin apps).
- `debugger`     — debugging UI widget.
- `shared`       — shared Vue components/composables for the two admin apps
  (consumed via the `@` alias, no package.json).
- `liveion` / `liveman` — the WebUIs embedded by the respective binaries.

Built assets are placed under `assets/<crate>/` and embedded at compile time
via `rust_embed::RustEmbed` when the `webui` feature is enabled.

## Build System

### Prerequisites

- Rust toolchain (stable; targets vary by platform).
- `pnpm` 12.16.1 or compatible; Node.js (CI uses `latest`).
- For WebUI builds: `pnpm install`.
- For native source features on Linux: `libcamera-dev`, `libv4l-dev`.
- For `source-wayland` (Linux-only): `libxkbcommon.so`, `libpipewire-0.3-dev`,
  `libopus-dev`; runtime needs an `Xwayland` binary and a PipeWire daemon —
  details in [liveion/AGENTS.md](liveion/AGENTS.md).
- For GStreamer-based tests: `gstreamer`, `gstreamer-rtsp-server`.
- For cross-compilation: `cross` from <https://github.com/cross-rs/cross>.

### Common Commands

```bash
pnpm install                                   # web dependencies
pnpm -r build                                  # build the web UIs
cargo build --release --all-targets --all-features   # all Rust targets (Linux; needs native deps)
cargo run --features=webui                     # main server with embedded WebUI
just run-cluster                               # local multi-node cluster
just build                                     # web + Rust release
```

### Feature Flags (Root Crate)

| Feature | Purpose |
|---|---|
| `webui` | embed static WebUI assets |
| `cascade` | cluster cascading via `libwish` |
| `net4mqtt` | MQTT-based tunneling |
| `recorder` | stream recording to storage (FS/S3) |
| `source` | auto-start configured media sources (base feature) |
| `source-sdp` / `source-rtsp` / `source-whep` | SDP-file / RTSP / WHEP-pull (static cascade-pull) sources |
| `source-all` | all source types except the Linux-only `source-wayland` |
| `source-wayland` | headless Wayland compositor desktop source (smithay + openh264, X11 via XWayland, PipeWire audio); see liveion/AGENTS.md |
| `target` | base for all `target-*` kinds (config, supervisor dispatch, virtual-subscriber ids); enables no kind on its own |
| `target-whip` / `target-rtp` / `target-rtsp` | static WHIP-push / RTP-UDP (multicast) / RTSP-push output targets; see liveion/AGENTS.md |
| `native-source` | required base for capture/encoder features |
| `capture-libcamera`, `capture-v4l2` | video capture backends |
| `capture-mock` | synthetic NV12 capture (no camera) for encoder testing |
| `encoder-v4l2-m2m`, `encoder-rdk`, `encoder-rkmpp`, `encoder-cedar` | encoder backends (cedar = Allwinner VE); see livehal/AGENTS.md |
| `encoder-opus` | pure-Rust Opus encoder (opus-rs in livehal) + `sine` test-tone capture |
| `capture-alsa` | ALSA microphone capture (Linux); implies `encoder-opus` |
| `native-rpi`, `native-generic-v4l2`, `native-rdk`, `native-rkmpp`, `native-cedar` | platform presets |
| `whepwright` | Playwright-based browser WHEP test harness |

Native capture/encoder features require Linux. On macOS/Windows CI the project
builds with `source-all,webui,net4mqtt,recorder,cascade,whepwright,target-whip,target-rtp,target-rtsp`
instead of `--all-features`.

### Cross-Compilation

Native backends cross-compile with `cross`, using per-platform ghcr images or
`*_SYSROOT` env overrides consumed by `livehal/build.rs` — images, env vars
and examples: [livehal/AGENTS.md](livehal/AGENTS.md). Size-optimized cross
builds: `just cross-pack-size <target> <features>` (see Deployment).

### Static musl diagnostics

Fully static diagnostics bundles (`x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`) are built in Alpine-based musl containers, not
the GNU cross images:

```bash
docker buildx build -f docker/Dockerfile.diagnostics-musl --target export \
  --output type=local,dest=dist/diagnostics \
  --build-arg TARGET=<target> --build-arg VERSION=<version> .
```

The export writes `live777-diagnostics(-sources)-<version>-<target>.tar.gz`
into `dist/diagnostics/`; binaries statically link FFmpeg/codecs, and the
source archive carries the Alpine aports `APKBUILD` recipes/patches plus the
upstream source downloads. The `build-diagnostics` / `build-diagnostics-musl` jobs
live in `.github/workflows/diagnostics.yml`, which builds on PRs touching
diagnostics packaging inputs (keeping Release off PRs so matrix job names
never render as raw `${{ matrix.* }}` placeholders); GitHub Release uploads
are tag-only. Validation must cover static linkage, relocation and real
media — not archive creation or `--version`: verify no dynamic FFmpeg/codec
deps, run the tools after moving the extracted directory, run a real
synthetic WHIP publish → WHEP decode smoke, and include `livewrk
--verify-window` with `whepprobe` beside it to exercise worker discovery.

## Runtime Architecture

- `live777` (`liveion`) is the edge SFU: WHIP publish + WHEP subscribe
  endpoints, admin/session APIs, Prometheus metrics, optional embedded WebUI.
- `liveman` sits in front of multiple `live777` nodes, proxies requests,
  manages cascade state, records via cluster policy, and stores recording
  indexes in a database.
- `livetwo` is the protocol-conversion engine used by `whipinto`/`whepfrom`
  and the `cascade` feature — see [livetwo/AGENTS.md](livetwo/AGENTS.md).
- `net4mqtt` exposes a local SOCKS proxy and tunnels traffic over MQTT for
  NAT/remote agents.

Configuration files: `conf/live777.toml` / `live777.toml` (main SFU),
`conf/liveman.toml` (cluster manager), `conf/livenil/` (cluster nil samples).
Important config sections: `http`, `stream`, `webrtc`, `ice_servers`, `auth`,
`recorder.storage`, `strategy`, `net4mqtt`.

There are no default ICE servers anywhere: liveion's `ice_servers` and
liveman's `extra_ice` are empty unless configured, so an out-of-the-box
deployment is LAN-only (host candidates only, no STUN/TURN advertised to
clients via Link headers).

## Code Organization

- Rust crate source lives in `src/` or `<crate>/src/`.
- `liveion/src/route/` — Axum route handlers (whip, whep, session, admin,
  stream, source, recorder, info, sdp, target).
- `liveion/src/forward/` — SFU forwarding core (media stats, RTCP metrics,
  DataChannel cables).
- `liveion/src/stream/` — stream manager + source adapters (provisioned
  streams, on-demand lifecycle, bitrate tiers, Wayland compositor).
- `liveion/src/target*.rs` — WHIP/RTP/RTSP output targets (static config
  plus the runtime `/api/targets` management API).
- `liveion/src/event.rs`, `src/recorder/`, `src/hook.rs` — lifecycle event
  bus, recording pipeline, hook scripts.

  → Deep-dives for all of the above: [liveion/AGENTS.md](liveion/AGENTS.md).

- `livetwo` — WHIP publish core, isolated FFmpeg decode workers, multicast
  transport, client DataChannel bridges →
  [livetwo/AGENTS.md](livetwo/AGENTS.md).
- `liveman/src/route/` — proxy/cascade/admin routes. The dashboard's streams
  table is SSE-pushed like liveion's: `GET /api/sse/streams`
  (`route/stream.rs::sse`, same `?nodes=` filter as `GET /api/streams/`)
  sends the merged cluster view on connect and re-sends it whenever
  `Storage`'s watch version bumps and on a 3 s idle cadence that drives the
  throttled lazy poll of poll-mode nodes; identical consecutive payloads are
  suppressed, and the loop selects on the server's `CancellationToken` so a
  streaming response cannot hold graceful shutdown open.
- `liveman/src/service/` — business logic (database, recordings index);
  `liveman/src/entity/` + `migration/` — Sea-ORM entities and migrations.
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

The project uses `cargo nextest` (`.config/nextest.toml`): the default
profile retries up to 4 times with exponential backoff; the `ci` profile has
1 retry, 120 s slow-timeout, `fail-fast = false`. Integration tests that use
FFmpeg, sockets, or browsers are forced serial (`serial-integration` group).

```bash
# full workspace with coverage, matching the CI feature set
cargo llvm-cov nextest --profile ci --workspace \
  --features source-all,webui,net4mqtt,recorder,cascade,rsmpeg,whepwright,rtsp,target-whip,target-rtp,target-rtsp \
  --lcov --output-path lcov.info

# without coverage
cargo nextest run --workspace
```

Integration test binaries in `tests/`:

- `matrix/` — end-to-end source × media-profile × player matrix (binary
  `matrix`): codec combos declared in `profile.rs`, sources in `source/`,
  players (livetwo+ffprobe, rsmpeg, Playwright) in `player/`, shared infra in
  `runner.rs`/`probe.rs`; includes the RTSP server push→pull round-trip
  (`rtsp_roundtrip_*`) and the RTSP→WHIP→WHEP→RTSP cycle (`rtsp_cycle_*`).
- `tests.rs` — liveion API smoke tests; `channel.rs`; `recorder.rs`.
- `livewrk_e2e.rs` — livewrk whip/whep subprocesses against in-process
  liveion, incl. rotating decode verification (needs `rsmpeg`).
- `diagnostics_e2e.rs` — whipsynth/whepprobe CLI end-to-end, incl. JSON and
  exit-code success/failure contracts (needs `rsmpeg`).
- `whepwright_e2e.rs` — real browser CLI output and failure-artifact
  contracts (needs `whepwright` + `rsmpeg`).
- `network_e2e.rs` — opt-in privileged STUN/TURN, netem and outage tests in
  private Linux namespaces: ignored by default, need `LIVE777_NETWORK_E2E=1`,
  root/CAP_NET_ADMIN, iproute2 and coturn; the Linux `build` job in
  `.github/workflows/rust.yml` installs these dependencies and runs them as
  root against its release/all-features build, preserving the
  Cargo/rustup/FFmpeg dynamic-library environment through sudo. They set
  `LIVE777_WEBRTC_DISABLE_MDNS=1` to skip mDNS candidate resolution in the
  livetwo client peers (`utils::webrtc::setting_engine`).
- `native_audio.rs` — native Opus audio source end-to-end (PT 111,
  sample-continuous RTP timestamps, opus-rs decode of every packet; ALSA
  unavailability/shutdown behavior with `capture-alsa`); gated on
  `native-source,encoder-opus`, compiles to an empty binary in the default CI
  feature set. Module-level test notes: liveion/AGENTS.md.
- `wayland_source.rs` — Wayland compositor source end-to-end (Linux,
  `source-wayland`; the client-content test also needs `rsmpeg`).

Notes:

- Tests that create local WebRTC peers set
  `LIVE777_WEBRTC_ICE_UDP_ADDRS=127.0.0.1:0` to force loopback ICE candidates.
- Playwright browser tests: `pnpm exec playwright install --with-deps
  chromium` and `export PLAYWRIGHT_BROWSERS_PATH=$PWD/.playwright`.
- mediamtx interop tests (`whep_mediamtx_pull_*`, `rtsp_push_mediamtx_*`,
  `rtsp_target_mediamtx_*` in the matrix binary, live777#212) need a mediamtx
  binary: `just mediamtx` downloads the pinned release into `target/`, or
  install it into `PATH`; `MEDIAMTX_BIN` overrides. The tests skip when no
  binary is found, and skip on Windows CI (GitHub-hosted Windows runners
  encode at ~0.03x realtime; same for the `whep_source_livetwo_*` relay
  matrix; the shared `runner::windows_ci()` helper carries the check).
- rsmpeg 0.18 only compiles against FFmpeg ≤ 8 headers (FFmpeg 9 privatized
  the AVCodec/AVFormatContext fields it touches); on newer distros build a
  minimal n8.0 — all 7 av libs, `--disable-everything
  --enable-decoder=h264 --enable-parser=h264` — into `target/ffmpeg8/install`
  and export `FFMPEG_PKG_CONFIG_PATH`/`PKG_CONFIG_PATH` to its
  `lib/pkgconfig` and `LD_LIBRARY_PATH` to its `lib`.

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

- **Docker**: multi-stage Dockerfiles in `docker/` (`live777-server`,
  `live777-client`, `live777-all`, `liveion`, `liveman`, `whipinto`,
  `whepfrom`, `net4mqtt`, `ffmpeg`, `gstreamer`), published to
  `ghcr.io/binbat/<app>`; `live777-client` is the self-contained FFmpeg +
  whipinto + whepfrom image (its FFmpeg build mirrors `Dockerfile.ffmpeg`).
  One app × architecture matrix builds each image once on a native
  amd64/arm64 runner, exporting a Docker tar for `docker load` and
  `scripts/docker-smoke.sh`, and pushes by digest only on version tags; PRs
  touching Docker inputs and pushes to the `docker` branch build+smoke
  without pushing. A single tag-only job then creates every multi-arch
  manifest from the tested digests (no arch-suffixed tags).
- **`live777-all`** is the all-in-one evaluation image (live777 + liveman +
  ffmpeg/ffprobe + whipinto + whepfrom + whipsynth + whepprobe + a
  `smoke-test` closed-loop WHIP→WHEP media check; `CMD ["live777"]`;
  `smoke-test --standalone` for a zero-setup first run, tuned by
  `SMOKE_SERVER_URL`/`SMOKE_TOKEN`/`SMOKE_PORT`). Because
  whepprobe/whipsynth need rsmpeg 0.18 — which cannot compile against
  Debian's field-privatized libav*-dev — the image builds upstream FFmpeg 8.1
  *shared* into `/opt/ffmpeg` (codec set mirrors `Dockerfile.live777-client`)
  with `FFMPEG_PKG_CONFIG_PATH` for the rust builder and `/opt/ffmpeg/lib`
  registered with ldconfig, so one build serves both the CLIs and the rsmpeg
  tools. The slim `live777-server`/`live777-client` images stay the pick for
  size-sensitive or production deployments.
- **systemd**: service units in `conf/live777.service` and
  `conf/liveman.service`.
- **Monitoring**: `compose.monitoring.yml` brings up Prometheus + Grafana
  scraping live777's always-on `/metrics`; scrape config in
  `conf/monitoring/prometheus.yml`, provisioned datasource and the `live777
  overview` dashboard under `conf/monitoring/grafana/`; `just monitoring-up`
  / `monitoring-down`. liveman has no Prometheus endpoint — scrape the
  live777 nodes directly.
- **Packages**: nFPM configs in `nfpm/` build `.deb`, `.rpm`, and Arch Linux
  packages; GitHub Actions upload them to releases.
- **Size-optimized builds are opt-in**: releases use the default `release`
  profile; for size-sensitive deployments use the `release-size` profile
  (`Cargo.toml`: fat LTO, 1 codegen unit, stripped, `panic=abort`) plus `upx
  --best --lzma` — `just build-size` / `pack-size` locally, `just
  cross-build-size` / `cross-pack-size <target> <features>` for cross builds,
  and device presets `just rpi-pack-size`, `rdk-pack-size` (needs
  `RDK_SYSROOT`), `v4l2-pack-size [target]` (armv7 default).
- **Releases**: `.github/workflows/release.yml` (tag pushes and the `test`
  branch) builds x86_64, aarch64, armv7, i686, riscv64, Android, Windows and
  macOS targets. Mainstream Linux/macOS/Windows targets also get a
  diagnostics bundle (whipsynth, whepprobe, livewrk + codec runtime, license
  notices, SHA-256-bound source archive) built by
  `.github/workflows/diagnostics.yml`. The live777-all Docker workflow runs
  `smoke-test --standalone` on each final arch image before publishing the
  multi-arch manifest.
- **Docs**: VitePress site in `docs/` (`pnpm run docs:dev` / `docs:build`);
  `docs:build` also emits `llms.txt`, `llms-full.txt` and per-page `.md`
  (English only) via `vitepress-plugin-llms`. `.github/workflows/docs.yml`
  checks the deployed `https://live777.pages.dev` with
  `docs/scripts/llms-txt-check.sh`.

## Useful Local Recipes (justfile)

```bash
just build            # web + Rust release build
just run              # cargo run --features=webui
just run-cluster      # local livenil cluster
just smoke            # standalone WHIP/WHEP encode/decode smoke test
just smoke-bridge     # synth -> WHIP -> WHEP -> RTP/SDP -> ffprobe decode
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
6. Run `just smoke` for the real standalone WHIP → WHEP decode check, or
   `just smoke-bridge` for whipinto synth → live777 → whepfrom RTP/SDP →
   ffprobe decode. Both build the required debug binaries with `rsmpeg`, use
   isolated temporary configuration and loopback ICE, and clean up their own
   processes; `CARGO_TARGET_DIR` is respected. The non-media safeguards are
   covered by `tests/smoke_port_conflict.sh` and `tests/smoke_audio_required.sh`.
7. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets --workspace --
   -D warnings`, and `cargo nextest run --workspace` before finishing.
