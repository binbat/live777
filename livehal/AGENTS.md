# livehal — Agent Guide

Notes for the native capture/encoder backend crate. The root
[AGENTS.md](../AGENTS.md) covers repo-wide conventions — read it first. When
you change behavior documented here, update this file.

## What Lives Here

- The C++ native pipeline (`native-pipeline/`) behind the capture/encoder
  features (`capture-libcamera`, `capture-v4l2`, `encoder-v4l2-m2m`,
  `encoder-rdk`, `encoder-rkmpp`, `encoder-cedar`); `native-source` is the
  required base feature. Native capture/encoder features require Linux.
- A pure-Rust audio path: `encoder-opus` (opus-rs, no C++ pipeline
  involvement) paired with the `sine` 440 Hz test-tone capture backend (no
  sound card — the test/CI path) or `capture-alsa` (ALSA microphone, Linux;
  implies `encoder-opus`).
- `encoder-cedar` is the Allwinner VE backend (e.g. A733/sun60iw2; H.264 +
  H.265); it supports DMA-BUF zero-copy from the V4L2 NV12M capture via
  per-plane dma-buf → VE IOVA mapping cached per capture buffer — the SDK's
  `nShareBufFd` shortcut is not used because it assumes one contiguous
  dma-buf while VIN exports the two NV12M planes as separate dma-bufs.
- Runtime encoder bitrate retuning (`EncoderBackend::setBitrate`) is
  supported by rkmpp, v4l2-m2m and cedar — used by liveion's adaptive
  bitrate controller and tier API (see
  [liveion/AGENTS.md](../liveion/AGENTS.md)).

## Cross-Compilation

`Cross.toml` configures `cross` images for `aarch64-unknown-linux-gnu` and
`armv7-unknown-linux-gnueabihf`. Per-platform cross images are published to
ghcr with the sysroot baked in; `*_SYSROOT` env vars override them with a
device-pulled sysroot when developing locally:

- Rockchip RKMPP (RK3588, RV1126B):
  `ghcr.io/binbat/crossbuilder-aarch64-rkmpp:latest` (sysroot at
  `/opt/rkmpp-sysroot`), or set `RKMPP_SYSROOT`.
- Raspberry Pi: `ghcr.io/binbat/crossbuilder-aarch64-rpi:latest`, or set
  `RPI_SYSROOT`. The image includes `libasound2-dev:arm64` for
  `capture-alsa`; use a device-pulled sysroot when the target requires a
  different camera ABI.
- RDK X5: set `RDK_SYSROOT` (no published image).
- Allwinner A733 / sun60iw2 (Orange Pi Zero 3W): no published image (the
  cedar SDK is device-only); set `SUNXI_SYSROOT` to a device-extracted
  sysroot, build the `docker/Dockerfile.cross-aarch64-sunxi` image from it,
  or build natively on the device (the system-installed SDK is auto-detected
  via `/usr/include/vencoder.h`). Recipe: `just cedar-cross-build`.

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

`build.rs` reads `RPI_SYSROOT`/`RDK_SYSROOT`/`RKMPP_SYSROOT`/`SUNXI_SYSROOT`
to configure `pkg-config` and linker paths.
