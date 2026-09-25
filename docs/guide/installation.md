# Installation

## Download Binary from GitHub

you can donwload binary from [here](https://github.com/binbat/live777/releases)

```bash
./live777
```

### Configuration

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```

## Docker

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

## ARM Boards (Raspberry Pi / RK3588 / RDK X5)

Releases after `v0.9.0` publish native builds with the **hardware capture +
encoder pipeline** (see [livehal](/guide/livehal)) baked in — a camera
streams without ffmpeg or any other helper process:

| Board | Release asset |
|-------|---------------|
| Raspberry Pi (64-bit OS) | `live777-<version>-aarch64-unknown-linux-gnu-rpi.tar.gz` |
| Rockchip RK3588 / RV1126B | `live777-<version>-aarch64-unknown-linux-gnu-rkmpp.tar.gz` |

```bash
TAG=v0.9.1   # must be newer than v0.9.0
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

