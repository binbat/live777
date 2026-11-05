# 安装部署

## 从 Gtihub 上下载二进制包直接运行

可以在这里下载我们编译好的二进制包 [here](https://github.com/binbat/live777/releases)

```bash
./live777
```

### 诊断工具包

诊断工具包的起始版本为 `v0.11.0`，更早的 Release 不包含这些资产。不同
Release 和目标平台的资产可能不同，下载前请确认所选已发布 Release 的实际
资产列表。包含诊断工具的 Release 会发布
`live777-diagnostics-<版本>-<目标>` 压缩包，内含使用 `rsmpeg` 构建的
`whipsynth`、`whepprobe` 和 `livewrk`。GNU/Linux、macOS 和 Windows 工具包
已包含动态 FFmpeg 运行时及其编解码器依赖，无需另外安装 FFmpeg。
`licenses/` 目录包含第三方许可证文本和依赖声明。

GNU/Linux 动态链接包会继续保留。资产列表中包含 musl 包的 Release 还会为
Alpine 等 musl Linux 系统提供全静态版本，其二进制不依赖动态 FFmpeg 或
编解码器库。Release 支持诊断工具包并不代表一定提供 musl 资产，下载前请
明确检查对应的 musl 目标。

每个诊断二进制包在同一 Release 中都有对应的源码包：Linux/macOS 为
`live777-diagnostics-sources-<版本>-<目标>.tar.gz`，Windows 为 `.zip`。
其中包含 Live777 源码快照、精确的 FFmpeg 源码、依赖源码下载和构建配方。
`MANIFEST.tsv` 记录二进制包名称、SHA-256、目标平台和版本，`REBUILD.md`
说明如何使用这些源码。缺少必要源码时，发布打包会直接失败。

请将 `TAG`（PowerShell 中为 `$env:TAG`）设置为已确认包含诊断资产的已发布
版本。以下示例要求显式设置该变量，不会回退到没有诊断资产的旧版本。

在 Linux 或 macOS 上下载并查看对应源码：

```bash
: "${TAG:?Set TAG to a published release with diagnostics assets}"
TARGET=x86_64-unknown-linux-gnu
curl -LO "https://github.com/binbat/live777/releases/download/${TAG}/live777-diagnostics-sources-${TAG}-${TARGET}.tar.gz"
tar xzf "live777-diagnostics-sources-${TAG}-${TARGET}.tar.gz"
cat "live777-diagnostics-sources-${TAG}-${TARGET}/MANIFEST.tsv"
cat "live777-diagnostics-sources-${TAG}-${TARGET}/REBUILD.md"
```

| 平台 | 目标 |
|------|------|
| Linux x86_64（GNU，动态） | `x86_64-unknown-linux-gnu` |
| Linux ARM64（GNU，动态） | `aarch64-unknown-linux-gnu` |
| Linux x86_64（musl，静态） | `x86_64-unknown-linux-musl` |
| Linux ARM64（musl，静态） | `aarch64-unknown-linux-musl` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

对于动态链接的 GNU/Linux 和 macOS 包，解压 `.tar.gz` 后请运行 `bin/` 中的
启动脚本。脚本会相对于解压目录定位随包的 `lib/`，因此整个目录可以移动；
不要直接运行 `.real` 二进制文件。

```bash
: "${TAG:?Set TAG to a published release with diagnostics assets}"
TARGET=x86_64-unknown-linux-gnu
curl -LO "https://github.com/binbat/live777/releases/download/${TAG}/live777-diagnostics-${TAG}-${TARGET}.tar.gz"
tar xzf "live777-diagnostics-${TAG}-${TARGET}.tar.gz"
cd "live777-diagnostics-${TAG}-${TARGET}"
./bin/whipsynth --version
./bin/whepprobe --version
./bin/livewrk --version
```

对于 musl 静态包，直接运行 `bin/` 中的二进制即可，不涉及动态启动脚本或
`.real` 二进制。解压目录可以整体移动，但解码验证需要让 `livewrk` 和
`whepprobe` 保持在同一目录。

```bash
: "${TAG:?Set TAG to a published release with the selected musl diagnostics asset}"
TARGET=x86_64-unknown-linux-musl
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
if (-not $env:TAG) {
    throw "Set TAG to a published release with diagnostics assets"
}
$Bundle = "live777-diagnostics-${env:TAG}-x86_64-pc-windows-msvc"
Expand-Archive -Path ".\$Bundle.zip" -DestinationPath .
cd ".\$Bundle"
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

