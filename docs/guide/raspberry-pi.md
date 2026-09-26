# Raspberry Pi

Deploy Live777 on a Raspberry Pi with a camera, using the **native hardware
pipeline**: libcamera capture + V4L2 M2M H.264 encoder, both inside the
`live777` process. No ffmpeg, no extra processes, no software encoding.

::: warning Do NOT use `ffmpeg + whipinto` on a Raspberry Pi
The generic `ffmpeg -> whipinto -> live777` path encodes video in software
(`libx264`) on the CPU. On a Pi this saturates all cores: expect low fps,
thermal throttling and growing latency. The `native-rpi` build below captures
and encodes in hardware — that is the supported way to stream a camera on
ARM boards (Raspberry Pi, Rockchip RK3588, Horizon RDK X5).
:::

Verified on a Raspberry Pi Zero 2 W (Debian 13, aarch64) with the OV5647
Camera Module v1. Any 64-bit Raspberry Pi OS with a working libcamera stack
(`rpicam-hello` shows a picture) works.

## Step 1: Install the native-rpi build

Releases after `v0.9.0` publish a prebuilt native Raspberry Pi tarball —
pick `live777-<version>-aarch64-unknown-linux-gnu-rpi.tar.gz` from the
[releases page](https://github.com/binbat/live777/releases):

```bash
TAG=v0.9.1   # the release you want; must be newer than v0.9.0
wget https://github.com/binbat/live777/releases/download/${TAG}/live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
tar xzf live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
cd live777-${TAG}-aarch64-unknown-linux-gnu-rpi
```

The tarball contains the `live777` binary, a `live777.toml` config template
and a `live777.service` systemd unit.

If your release has no `-rpi` asset (the native build was added after
`v0.9.0`), build from source instead — see [livehal](./livehal.md#build),
or cross-compile with `just rpi-cross-build`.

## Step 2: Configure the camera source

Edit `live777.toml` — one provisioned stream with a camera source and a
quality ladder (source quality tiers, first shipped in
[#437](https://github.com/binbat/live777/pull/437)):

```toml
[stream.pi-cam]
[[stream.pi-cam.sources]]

[stream.pi-cam.sources.capture]
backend = "libcamera"
device = "0"
width = 1296        # OV5647 native 2x2-binned mode: full field of view
height = 972
fps = 30
pixel_format = "yuv420"
prefer_dmabuf = true  # DMA-BUF zero-copy capture → encode (optional)

[stream.pi-cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 2000000   # ceiling — adaptive bitrate (AIMD) only lowers from here
profile = "baseline"
level = "4.0"
gop = 60
prefer_dmabuf = true  # must be set on both sides for zero-copy

[stream.pi-cam.sources.output]
payload_type = 96
clock_rate = 90000

# Quality ladder, switchable at runtime. A capture block rebuilds the
# pipeline (sub-second gap, subscribers stay connected); an encoder-only
# tier retunes in place. An omitted encoder.bitrate is derived from the
# tier's geometry (~0.07 bpp).
[[stream.pi-cam.sources.tiers]]
name = "std"
capture = { width = 1296, height = 972, fps = 30 }
encoder = { bitrate = 2000000 }
[[stream.pi-cam.sources.tiers]]
name = "maxres"
capture = { width = 1920, height = 1080, fps = 20 }
encoder = { bitrate = 3000000 }
[[stream.pi-cam.sources.tiers]]
name = "lowlatency"
capture = { width = 640, height = 480, fps = 60 }
encoder = { bitrate = 1000000 }
[[stream.pi-cam.sources.tiers]]
name = "lowbw720"
capture = { width = 1280, height = 720, fps = 10 }
encoder = { bitrate = 500000 }
[[stream.pi-cam.sources.tiers]]
name = "lowbwtiny"
capture = { width = 320, height = 240, fps = 10 }
encoder = { bitrate = 150000 }
```

Adaptive bitrate is on by default: an AIMD controller follows WHEP
subscriber RTCP feedback and retunes the running encoder, with the active
tier's bitrate as its ceiling and the lowest tier as its floor. See
[livehal](./livehal.md#adaptive-bitrate-experimental) for the semantics.

The `prefer_dmabuf` pair enables DMA-BUF zero-copy: captured frames are
queued to the hardware encoder as dma-bufs, eliminating the two full-frame
CPU copies per frame — e.g. 1296x972@30 drops from ~55% to ~12% of one
core on a Zero 2 W. It is optional — without it (or if the driver cannot
import the buffer) the pipeline uses the CPU-copy path. See
[livehal — Zero-copy (DMA-BUF)](./livehal.md#zero-copy-dma-buf) for the
full benchmark table.

## Step 3: Run

```bash
./live777 --config live777.toml
```

Or install the bundled systemd unit:

```bash
sudo cp live777 /usr/local/bin/
sudo cp live777.toml /etc/live777.toml
sudo cp live777.service /etc/systemd/system/
sudo systemctl enable --now live777
```

## Step 4: Play

Open `http://<pi-ip>:7777/` in a browser — the dashboard lists the
provisioned `pi-cam` stream; click to play it over WHEP.

## Step 5: Switch quality at runtime

From the dashboard's **Bitrate** dialog, or via the API:

```bash
# ladder rung
curl -X POST http://<pi-ip>:7777/api/sources/pi-cam/tier \
  -H 'Content-Type: application/json' -d '{"tier": "lowlatency"}'

# current drive mode (adaptive/fixed) and encoder rate
curl http://<pi-ip>:7777/api/sources/pi-cam/bitrate
```

## Troubleshooting

- **~100% CPU, low fps, latency keeps growing** — you are software-encoding
  (an ffmpeg/`libx264` pipeline). Switch to the `native-rpi` build above;
  the hardware encoder costs ~20–65% of *one* core depending on resolution.
- **Camera busy / black picture** — another process holds the sensor
  (`rpicam-hello`, motion, ...). Check with `rpicam-hello --timeout 1`.
- **fps below the configured value** — the sensor mode clamps it; see the
  per-mode ceilings and the 60 fps reference config in
  [livehal — Raspberry Pi notes](./livehal.md#raspberry-pi-notes).
