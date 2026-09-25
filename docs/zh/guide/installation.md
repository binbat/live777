# 安装部署

## 从 Gtihub 上下载二进制包直接运行

可以在这里下载我们编译好的二进制包 [here](https://github.com/binbat/live777/releases)

```bash
./live777
```

### 使用配置

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```

## Docker

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

## ARM 开发板（树莓派 / RK3588 / RDK X5）

`v0.9.0` 之后的 Release 会发布内置**硬件采集 + 编码管线**（见 [livehal](/zh/guide/livehal)）的原生构建——摄像头推流不需要 ffmpeg 或任何辅助进程：

| 开发板 | Release 资产 |
|-------|---------------|
| 树莓派（64 位系统） | `live777-<版本>-aarch64-unknown-linux-gnu-rpi.tar.gz` |
| 瑞芯微 RK3588 / RV1126B | `live777-<版本>-aarch64-unknown-linux-gnu-rkmpp.tar.gz` |

```bash
TAG=v0.9.1   # 必须晚于 v0.9.0
wget https://github.com/binbat/live777/releases/download/${TAG}/live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
tar xzf live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
cd live777-${TAG}-aarch64-unknown-linux-gnu-rpi
./live777 --config live777.toml
```

完整的摄像头部署步骤见[树莓派部署指南](/zh/guide/raspberry-pi)。RDK X5 请使用 `native-rdk` 预设从源码构建（需要 `RDK_SYSROOT`）。

::: warning
这些开发板做软编码效果很差——**不要**在上面走通用的 `ffmpeg -> whipinto`（`libx264`）路径，请使用上面的原生构建。
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

