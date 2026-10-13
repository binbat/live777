# PX4 + Gazebo → live777 Streaming Demo (Operator Handoff)

This document hands off a working local demo environment to another engineer or
AI agent. It contains everything needed to run, verify, tune, and extend a
pipeline that streams a PX4 SITL + Gazebo simulation out through
[live777](https://github.com/binbat/live777) over WebRTC (WHEP), with no
physical display anywhere in the loop.

Audience note: this is an **ops/dev environment doc**, not upstream
documentation. Paths are host-specific (the host is an Arch Linux machine with
an Intel UHD 630 iGPU; live777 itself runs from a git worktree). Everything
here was validated end-to-end on 2026-10-12/13.

---

## 1. What this environment does

Two WebRTC streams published by one host `live777` process:

| Stream | Content | Path |
|---|---|---|
| `demo` | The Gazebo GUI (Qt6 window: entity tree, panels) | Jetty GUI runs as a **Wayland client** of live777's built-in headless compositor (feature `source-wayland`, SHM + dmabuf/EGL), which composites and encodes H.264 |
| `fpv` | The drone's **onboard camera** (FPV view of the `forest` world) | Gazebo camera sensor rendered server-side (`--headless-rendering`) → PX4's built-in `GstCameraSystem` plugin (gstreamer: appsrc→x264enc→rtph264pay→udpsink) → plain RTP/UDP H.264 to `127.0.0.1:5600` → live777's **SDP-file source** (feature `source-sdp`) |

Everything runs in **one** docker container (`px4-jetty:resolute`, Ubuntu 26.04
+ PX4 v1.17 + Gazebo Jetty) plus the host's live777.

Watch: `http://192.168.111.17:7778/` (LAN) or
`http://[fd7a:115c:a1e0::481c:1650]:7778/` (Tailscale) — no auth configured in
this demo config.

## 2. Files in this directory

| File | Role |
|---|---|
| `Dockerfile.px4-jetty` | Builds the `px4-jetty:resolute` image (Ubuntu 26.04 + build deps + Gazebo Jetty + Qt wayland plugins + gstreamer) |
| `run-demo.sh` | Starts the container: builds PX4 (incremental, cached on host), starts the gz server + PX4 + camera streaming + wayland GUI |
| `live777.toml` | live777 demo config: both streams, LAN+Tailscale ICE candidates |
| `fpv.sdp` | Static SDP describing the GstCameraSystem RTP stream (H.264 pt 96 on port 5600) |

Files NOT in this directory but required (host paths):

- live777 worktree: `/home/metal/Code/live777-wayland` (branch
  `feat/wayland-source`, pushed to `origin`; build with
  `cargo build --features source-wayland,webui,source-sdp --bin live777`)
- PX4 source: `/tmp/px4-1.17` (release/1.17, shallow clone; **lost on reboot** —
  see §7.1)
- Runtime dir for the wayland socket: `/run/user/1000` (host
  `$XDG_RUNTIME_DIR`)
- Shared dir for the SDP file: `/tmp/live777-share` (created by run-demo.sh)

## 3. Runbook

### 3.1 One-time host setup

```bash
# GPU access for the current user (Intel i915; compositor + container GL)
sudo setfacl -m u:$USER:rw /dev/dri/renderD128 /dev/dri/card0
# or permanently: sudo usermod -aG render $USER  (re-login needed)

# PX4 source (only if /tmp/px4-1.17 is gone)
git clone --depth 1 --branch release/1.17 --recurse-submodules \
  --shallow-submodules https://github.com/PX4/PX4-Autopilot.git /tmp/px4-1.17
# then fix shallow-clone submodule breakage — see §6.3.

# Docker image (~25 min; the OSRF apt mirror is slow, ~500 KB/s)
docker build -f Dockerfile.px4-jetty -t px4-jetty:resolute .
```

### 3.2 Start live777 (host)

```bash
cd /home/metal/Code/live777-wayland
nohup ./target/debug/live777 -c /path/to/live777.toml > /tmp/live777-demo.log 2>&1 &
# verify: curl http://localhost:7778/api/streams/  → "demo" and "fpv" listed
```

Note: live777's `fpv` SDP source binds UDP 5600 at startup. If it fails with
"Address already in use", see §6.7.

### 3.3 Start the simulator stack (container)

```bash
./run-demo.sh   # foreground-ish; logs to stdout. Give it 3-4 min first time.
```

Order of events inside the container (all automated):
1. `ninja` incremental PX4 build (build dir persisted in `/tmp/px4-1.17/build`)
2. gz server on `forest.sdf` with PX4's `server.config` + `--headless-rendering`
3. PX4 SITL starts, spawns `x500_mono_cam`
4. `GstCameraSystem` detects the camera image topic and streams H.264/RTP to
   `127.0.0.1:5600`
5. the Gazebo GUI (`gz sim -g`) attaches to live777's wayland compositor

### 3.4 Verify

```bash
# streams publishing (bytes/packets growing)
curl -s http://localhost:7778/api/streams/ | python3 -m json.tool | grep -A3 '"bitrate"'

# grab a frame from a stream (writes frame.png)
./target/debug/whepfrom -o "rtp://localhost?video=9000" --sdp-file /tmp/x.sdp \
  -w http://localhost:7778/whep/fpv &  # wait for the sdp file, then:
ffmpeg -y -protocol_whitelist rtp,file,udp -i /tmp/x.sdp -vframes 1 frame.png
```

Expected: `fpv` shows a ground-level forest view (trees, grass), `demo` shows
the Gazebo GUI window chrome.

## 4. Interfaces worth knowing

### 4.1 GstCameraSystem ↔ live777 (the `fpv` stream)

- Plugin source: `src/modules/simulation/gz_plugins/gstreamer/GstCameraSystem.cpp`
  in PX4 v1.17. It subscribes gz-transport camera topics matching
  `/world/*/model/*/link/*/sensor/*/image`, then:
  `appsrc → videoconvert → x264enc → rtph264pay → udpsink`.
- Encoder: H.264, bitrate 4000 kbps, `tune=zerolatency`, `key-int-max=30`
  (≈1 IDR/s @30fps), MTU 1400; **`config-interval=1` (SPS/PPS in-band with
  every IDR — this is what lets late-joining decoders sync within a second)**.
- Destination: `127.0.0.1:5600` by default; override with env
  `PX4_VIDEO_HOST_IP`.
- live777 side: static SDP file (`fpv.sdp` here) consumed by the `source-sdp`
  feature. No backchannel — subscriber PLI never reaches gstreamer; the 1 Hz
  IDR cadence covers sync.
- To re-point live777 at a changed SDP **without restarting**: 
  `curl -X DELETE localhost:7778/api/sources/fpv` then
  `curl -X POST localhost:7778/api/sources/fpv -H 'Content-Type: application/json' -d '{"url":"file:///path/fpv.sdp"}'`.

### 4.2 Wayland compositor ↔ Gazebo GUI (the `demo` stream)

- live777 creates a wayland display socket `live777-demo` in
  `$XDG_RUNTIME_DIR`; the container mounts `/run/user/1000` and sets
  `WAYLAND_DISPLAY=live777-demo` + `QT_QPA_PLATFORM=wayland`.
- Clients: SHM and dmabuf (`zwp_linux_dmabuf_v1`, EGL/GBM on `/dev/dri`).
  live777 composites (GLES on i915; CPU fallback) and encodes via openh264.
- Known limitation: the GUI's **3D viewport stays blank** — Ogre2's offscreen
  render falls back to its EGL-pbuffer path, which crashes on a "Full Screen"
  config option (unfixed in ogre-next 2.3.1 and 2.3.3; see
  <https://gazebosim.org/docs/latest/troubleshooting/>). GUI chrome (Qt panels)
  is unaffected. The camera-based `fpv` stream is the intended way to watch the
  3D scene.

### 4.3 live777 endpoints

- WHEP play: `http://<host>:7778/whep/<stream>` (`demo`, `fpv`)
- Admin REST: `/api/streams/`, `/api/sources/{stream}` (GET/POST/DELETE)
- WebUI: `http://<host>:7778/` (streams list + player)

## 5. Tuning knobs

| What | Where |
|---|---|
| World (map) | `run-demo.sh`: `forest.sdf` → any of `/tmp/px4-1.17/Tools/simulation/gz/worlds/*.sdf` (`baylands`, `aruco`, `lawn`, `underwater`, ...) |
| Vehicle/camera model | `PX4_SIM_MODEL=gz_x500_mono_cam` → `gz_x500_depth`, `gz_x500_vision`, ... (a matching camera topic is auto-found) |
| Camera resolution/fps | `Tools/simulation/gz/models/mono_cam/model.sdf` (`<image>` and `<update_rate>`) |
| fpv bitrate/quality | `GstCameraSystem.cpp` x264enc `bitrate`/`quantizer` (needs rebuild) |
| demo stream size/fps/bitrate | `live777.toml`: `wayland://...?width=&height=&fps=&bitrate=` |
| Add objects to the scene | edit the world `.sdf` (add `<include>` with a fuel URI or local model) |

## 6. Pitfalls (all hit during bring-up — read before touching)

1. **OSRF apt mirror is slow** (~500 KB/s here). Never install gz packages in an
   ephemeral container — that's why the image exists. Rebuilding the image is
   cheap if you only add new layers at the END of the Dockerfile (reordering
   invalidates the Jetty layer and costs you a full re-download).
2. **PX4 shallow-clone submodule breakage**: `git clone --recurse-submodules
   --shallow-submodules` leaves several submodules at wrong commits or empty.
   Symptom: cmake/ninja errors like "No SOURCES given to target: heatshrink" or
   an interactive "submodule is not in the recommended version" prompt that
   hangs forever without a TTY. Fix (host side):
   ```bash
   cd /tmp/px4-1.17
   git submodule status | grep '^+'   # each mismatched one:
   #   git -C <path> fetch --depth 1 origin <recorded-sha> && git -C <path> checkout -q <sha>
   git -C src/lib/heatshrink/heatshrink checkout -f HEAD      # empty worktree
   git -C src/modules/mavlink/mavlink submodule update -f --init pymavlink
   ```
3. **empy 4.x breaks PX4's message generators** (`module 'em' has no attribute
   'RAW_OPT'`). The image pins `empy>=3.3,<4` in its own pip layer FIRST —
   combined installs resolved 4.x despite the pin.
4. **`libopencv-dev` is required** for the `optical_flow` gz plugin's cmake;
   without it the whole PX4 configure fails.
5. **gstreamer dev packages must exist at cmake configure time**, else
   `GstCameraSystem` is silently skipped (`pkg_check_modules` guard). The
   run-demo.sh re-runs `cmake -S . -B build/px4_sitl_default` before `ninja`
   for exactly this reason.
6. **Jetty moved the server config to an env var**: Garden's
   `gz sim -s --server-config <file>` is gone; Jetty reads
   `GZ_SIM_SERVER_CONFIG_PATH` (gz_env.sh exports it too). Without the config,
   the `Sensors` system never loads and camera topics never appear.
7. **PX4's pxh console spam is one giant never-newlined line** — it breaks
   `grep` pipelines (block buffering / giant line buffering). The demo scripts
   allocate a TTY (`docker run -t`) instead of filtering.
8. **`docker run -i` fails in non-tty contexts** ("cannot attach stdin to a
   TTY-enabled container"); use `-t` only. For background GUI launches inside a
   running container, use `docker exec -d` — a backgrounded process inside a
   foreground `docker exec` gets killed when the exec returns.
9. **Two `--network host` containers collide invisibly**: X11 abstract sockets
   (`/tmp/.X11-unix/X99`) AND gz-transport discovery are per-netns-shared, so a
   second sim stack steals the display number and the world. Also: PX4's rc
   script *reuses any running world it discovers* (`gz topic -l`), which can
   silently cross-wire two stacks. Keep one stack per netns, or drop
   `--network host` and push RTP to the host's LAN IP instead of 127.0.0.1.
10. **A "killed" backgrounded live777 can survive as an orphan** holding UDP
    ports; the next live777 then fails to bind the SDP source port ("Address
    already in use") while HTTP looks fine. `pgrep -a live777` before
    restarting; use the admin API to restart a single source when possible.
11. **Two live777 writing the same log via `tee` in a size-capped harness**:
    long-running `live777 -vv` produces >16 MiB of logs — redirect to a file,
    not a pipe.
12. `weston-flower` draws once then waits for frame callbacks (looks frozen);
    `weston-smoke` animates but its window is effectively transparent. For
    quick wayland-client sanity checks use `weston-terminal` or
    `weston-simple-egl`.
13. On this host `/tmp` is tmpfs (63G) and `$XDG_RUNTIME_DIR` is
    `/run/user/1000`. Reboot wipes `/tmp/px4-1.17`, `/tmp/live777-share`, and
    any running containers' state.

## 7. Current state snapshot (as of handoff)

- `px4-jetty:resolute` image: built (Ubuntu 26.04.1, Gazebo Jetty 10.5.0,
  ogre-next 2.3.3, gstreamer 1.26, Qt6 + qt6-wayland).
- `/tmp/px4-1.17`: PX4 release/1.17, submodules fixed (§6.2), build cache warm.
- Host live777: running from the worktree on `[::]:7778`, streams `demo` +
  `fpv` configured.
- The `feat/wayland-source` branch (6 commits) carries the whole
  `source-wayland` feature incl. dmabuf/EGL support, docs (en+zh), and tests
  (151 liveion tests + WHEP e2e green).

## 8. If you want to extend this

- Make the drone fly: PX4's nsh console runs on the container's TTY; inject
  `commander takeoff` via `docker exec px4-jetty ...` into the px4 stdin (the
  demo's `docker run -t` keeps it), or use mavsdk/QGC against
  `udp://127.0.0.1:14550` (host network).
- Follow-ups on the live777 side live in the feature branch: input injection
  (keyboard/mouse over DataChannel → smithay seat), VAAPI hardware encode,
  subsurface/popup compositing.
- Fixing Gazebo's 3D viewport on Wayland means patching ogre-next's
  `EglPBufferSupport::setConfigOption` (accept/ignore `Full Screen`) or
  upstreaming a fix; not required for the FPV path.
