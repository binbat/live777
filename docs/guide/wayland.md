# Wayland Compositor Source

live777 can act as a **headless Wayland display server**: ordinary GUI
applications render into it, and the composited desktop is H.264-encoded and
published as a WebRTC stream. No physical display, GPU, or X11/Wayland session
is required.

This is a stream *source*: build live777 with the `source-wayland` feature
(Linux only, not part of `source-all`) and give a stream a `wayland://` URL:

```bash
cargo build --release --bin live777 --features source-wayland
```

Linking needs the `libxkbcommon` shared library (e.g. `libxkbcommon-dev` on
Debian/Ubuntu); no other system dependencies.

## Configuration

```toml
# The stream is provisioned: it exists at startup and appears in the
# API/Dashboard even before any application connects.
[stream.desktop]
[[stream.desktop.sources]]
# wayland://<socket-name>?width=&height=&fps=&bitrate=&xwayland=&xdisplay=
# All parameters are optional (defaults: socket live777-desktop,
# 1280x720@30, 2 Mbps, xwayland=1, xdisplay auto-assigned).
url = "wayland://desktop-0?width=1920&height=1080&fps=30&bitrate=4000000"
```

The compositor creates the Wayland socket `$XDG_RUNTIME_DIR/desktop-0` at
startup, so **`XDG_RUNTIME_DIR` must be set** when live777 starts. Point any
Wayland application at the display by name:

```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)
live777 &                                  # serves the stream
WAYLAND_DISPLAY=desktop-0 my-application   # renders into the stream
```

Subscribers watch the stream like any other: `http://localhost:7777/` in a
browser, or any WHEP player. Until the first application connects, the stream
carries a moving test pattern, so the publish is alive immediately and
subscribing never has to wait for a client.

## X11 applications (XWayland)

X11 applications work too: with `xwayland=1` (the default) the compositor
spawns an **XWayland** server and acts as its X11 window manager. This needs
an `Xwayland` binary on `PATH` (e.g. the `xwayland`/`xorg-xwayland` package);
without one, X11 support is skipped with a warning and native Wayland clients
keep working.

The display number is picked automatically and logged at startup
(`XWayland ready, X11 clients use DISPLAY=:N`); pin it with `xdisplay=`:

```toml
url = "wayland://desktop-0?xdisplay=0"
```

```bash
DISPLAY=:0 xterm &      # renders into the same stream as Wayland clients
```

Window management is deliberately minimal — this is a virtual monitor, not a
desktop: windows keep the geometry they ask for (there is no pointer input to
move or resize them), fullscreen/maximize requests map to the full canvas, and
the last mapped window paints on top. X11 and Wayland clients can be mixed on
one stream; X11 windows paint above xdg toplevels, and override-redirect
windows (menus, tooltips) on top of everything.

X11 sockets live in `/tmp/.X11-unix/` (created if missing). To reach X11
clients inside a container, bind-mount it through (or share the network
namespace for the abstract socket):

```bash
docker run -v /tmp/.X11-unix:/tmp/.X11-unix -e DISPLAY=:0 ...
```

Set `LIVE777_WAYLAND_XWAYLAND_LOG=1` to inherit XWayland's stderr instead of
discarding it (useful when an X11 client stays black).

## Running as a systemd service

System services have no `XDG_RUNTIME_DIR` by default. Give the service a
runtime directory and point both live777 and the applications at it:

```ini
[Service]
RuntimeDirectory=live777
Environment=XDG_RUNTIME_DIR=/run/live777
```

`RuntimeDirectory=live777` creates `/run/live777` owned by the service user;
clients then connect with `XDG_RUNTIME_DIR=/run/live777
WAYLAND_DISPLAY=desktop-0`.

## Current limitations

- **GPU clients need a working GPU/EGL driver on the host** — with one,
  `zwp_linux_dmabuf_v1` is advertised and GL clients (Qt Quick, browsers,
  mpv, Gazebo) render straight into the stream; without one the compositor
  falls back to CPU compositing and only SHM (wl_shm) clients work. The DRM
  device defaults to `/dev/dri/renderD128` then `/dev/dri/card0`, overridable
  with `LIVE777_WAYLAND_DRM_DEVICE`. The same applies to X11 GL applications
  (XWayland glamor → DRI3 → dmabuf).
- **No input injection** — a seat with keyboard/pointer exists so clients can
  bind it, but no input events are generated. X11 windows are therefore not
  focusable/movable; keyboard focus stays with the last mapped X11 window.
- **No audio** — the stream is video-only.
- **Subsurfaces and xdg popups are not composited** — only xdg toplevels are
  drawn (popups are configured but skipped). X11 override-redirect windows
  *are* composited, so X11 menus/tooltips show up.
- **X11 window decorations are client-side only** — the built-in XWM draws no
  title bars, and there is no clipboard sync between X11 and Wayland clients.
- **Buffer scale ≠ 1 and buffer transforms are drawn as-is** (best effort).

## H.264 patent note

The encoder is OpenH264. H.264 is patent-encumbered in some jurisdictions:
Cisco's patent license covers only their *prebuilt* binaries — when you build
live777 from source, responsibility for codec patent licensing sits with
whoever distributes the binary.
