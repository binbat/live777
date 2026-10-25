# Installation

## Download Binary from GitHub

you can donwload binary from [here](https://github.com/binbat/live777/releases)

```bash
./live777
```

### Diagnostics bundle

Releases also publish `live777-diagnostics-<version>-<target>` archives with
`whipsynth`, `whepprobe` and `livewrk`, all built with `rsmpeg`. The bundle
includes the dynamic FFmpeg runtime and its codec dependencies; a separate
FFmpeg installation is not required.

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

On Linux and macOS, extract the `.tar.gz` and run the launchers in `bin/`. They
locate the bundled `lib/` relative to the extracted directory, so the whole
directory can be moved. Do not invoke the `.real` binaries directly.

```bash
TAG=v0.11.0
TARGET=x86_64-unknown-linux-gnu
curl -LO "https://github.com/binbat/live777/releases/download/${TAG}/live777-diagnostics-${TAG}-${TARGET}.tar.gz"
tar xzf "live777-diagnostics-${TAG}-${TARGET}.tar.gz"
cd "live777-diagnostics-${TAG}-${TARGET}"
./bin/whipsynth --version
./bin/whepprobe --version
./bin/livewrk --version
```

On Windows, extract the `.zip`; the tools, FFmpeg/codec DLLs and x64 MSVC
runtime DLLs (`vcruntime140*`, `msvcp140*`, `concrt140*` and, when present,
`vccorlib140*`) are together in `bin/`. No separate Visual C++ Redistributable
installation is required:

```powershell
Expand-Archive .\live777-diagnostics-v0.11.0-x86_64-pc-windows-msvc.zip
cd .\live777-diagnostics-v0.11.0-x86_64-pc-windows-msvc
.\bin\whipsynth.exe --version
.\bin\whepprobe.exe --version
.\bin\livewrk.exe --version
```

### Configuration

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```

## Docker

For a quick start, use the all-in-one image — it bundles `live777`,
`liveman`, FFmpeg, the WHIP/WHEP tools and a `smoke-test` self-check:

```sh
docker run -d --name live777 --rm --network host ghcr.io/binbat/live777-all:latest
docker exec live777 smoke-test
```

See [Getting Started](/guide/getting-started) for the full verification
flow.

For production deployments, pick a slimmer image per role:

| Image | Contents |
|-------|----------|
| `ghcr.io/binbat/live777-server` | `live777` + `liveman` |
| `ghcr.io/binbat/liveion` | `live777` only |
| `ghcr.io/binbat/liveman` | `liveman` only |
| `ghcr.io/binbat/live777-client` | FFmpeg + `whipinto` + `whepfrom` client tools |
| `ghcr.io/binbat/whipinto` / `whepfrom` / `net4mqtt` | Single-tool images |

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

## ARM Boards (Raspberry Pi / RK3588 / RDK X5)

Releases from `v0.10.0` onward publish native builds with the **hardware capture +
encoder pipeline** (see [livehal](/guide/livehal)) baked in — a camera
streams without ffmpeg or any other helper process:

| Board | Release asset |
|-------|---------------|
| Raspberry Pi (64-bit OS) | `live777-<version>-aarch64-unknown-linux-gnu-rpi.tar.gz` |
| Rockchip RK3588 / RV1126B | `live777-<version>-aarch64-unknown-linux-gnu-rkmpp.tar.gz` |

```bash
TAG=v0.10.0   # the first release with native builds; or any newer one
wget https://github.com/binbat/live777/releases/download/${TAG}/live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
tar xzf live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
cd live777-${TAG}-aarch64-unknown-linux-gnu-rpi
./live777 --config live777.toml
```

See the [Raspberry Pi deployment guide](/guide/raspberry-pi) for the full
camera setup. For RDK X5, build from source with the `native-rdk` preset
(requires `RDK_SYSROOT`).

::: warning
These boards software-encode poorly — do **not** use the generic
`ffmpeg -> whipinto` path with `libx264` on them; use the native builds
above.
:::

## Cargo

```bash
cargo install --git http://github.com/binbat/live777 live777 --bin=whipinto
cargo install --git http://github.com/binbat/live777 live777 --bin=whepfrom
```

## Debian / Ubuntu

```bash
wget https://github.com/binbat/live777/releases/download/latest/live777_<X>.<Y>.<Z>_amd64.deb
dpkg -I live777_<X>.<Y>.<Z>_amd64.deb
systemctl start live777
```

## Centos / Fedora

```bash
wget https://github.com/binbat/live777/releases/download/latest/live777-<X>.<Y>.<Z>.x86_64.rpm
rpm -i live777-<X>.<Y>.<Z>.x86_64.rpm
systemctl start live777
```

## Archlinux

```bash
wget https://github.com/binbat/live777/releases/download/latest/live777-<X>.<Y>.<Z>-x86_64.pkg.tar.zst
pacman -U live777-<X>.<Y>.<Z>-x86_64.pkg.tar.zst
systemctl start live777
```

## Windows

**Winget**

```bash
winget install live777
winget install whipinto
winget install whepfrom
```

