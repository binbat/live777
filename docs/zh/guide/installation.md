# 安装部署

## 从 Gtihub 上下载二进制包直接运行

可以在这里下载我们编译好的二进制包 [here](https://github.com/binbat/live777/releases)

```bash
./live777
```

### 诊断工具包

Release 同时发布 `live777-diagnostics-<版本>-<目标>` 压缩包，包含使用
`rsmpeg` 构建的 `whipsynth`、`whepprobe` 和 `livewrk`。工具包已包含动态
FFmpeg 运行时及其编解码器依赖，无需另外安装 FFmpeg。

| 平台 | 目标 |
|------|------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

在 Linux 和 macOS 上解压 `.tar.gz` 后，请运行 `bin/` 中的启动脚本。脚本会
相对于解压目录定位随包的 `lib/`，因此整个目录可以移动；不要直接运行
`.real` 二进制文件。

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

在 Windows 上解压 `.zip` 后，工具、FFmpeg/编解码器 DLL 和 x64 MSVC 运行时
DLL（`vcruntime140*`、`msvcp140*`、`concrt140*`，以及存在时的
`vccorlib140*`）均位于 `bin/`，无需另外安装 Visual C++ Redistributable：

```powershell
Expand-Archive .\live777-diagnostics-v0.11.0-x86_64-pc-windows-msvc.zip
cd .\live777-diagnostics-v0.11.0-x86_64-pc-windows-msvc
.\bin\whipsynth.exe --version
.\bin\whepprobe.exe --version
.\bin\livewrk.exe --version
```

### 使用配置

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```

## Docker

快速上手推荐 all-in-one 镜像——它打包了 `live777`、`liveman`、FFmpeg、
WHIP/WHEP 工具和 `smoke-test` 自检命令：

```sh
docker run -d --name live777 --rm --network host ghcr.io/binbat/live777-all:latest
docker exec live777 smoke-test
```

完整的验流流程见[快速开始](/zh/guide/getting-started)。

生产部署可按角色选择更精简的镜像：

| 镜像 | 内容 |
|-------|------|
| `ghcr.io/binbat/live777-server` | `live777` + `liveman` |
| `ghcr.io/binbat/liveion` | 仅 `live777` |
| `ghcr.io/binbat/liveman` | 仅 `liveman` |
| `ghcr.io/binbat/live777-client` | FFmpeg + `whipinto` + `whepfrom` 客户端工具 |
| `ghcr.io/binbat/whipinto` / `whepfrom` / `net4mqtt` | 单工具镜像 |

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

## ARM 开发板（树莓派 / RK3588 / RDK X5） {#arm-boards}

自 `v0.10.0` 起的 Release 会发布内置**硬件采集 + 编码管线**（见 [livehal](/zh/guide/livehal)）的原生构建——摄像头推流不需要 ffmpeg 或任何辅助进程：

| 开发板 | Release 资产 |
|-------|---------------|
| 树莓派（64 位系统） | `live777-<版本>-aarch64-unknown-linux-gnu-rpi.tar.gz` |
| 瑞芯微 RK3588 / RV1126B | `live777-<版本>-aarch64-unknown-linux-gnu-rkmpp.tar.gz` |

```bash
TAG=v0.10.0   # 首个包含原生构建的版本，或更新的 Release
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

