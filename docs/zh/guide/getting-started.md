# 快速开始

## 最小化

### 可以使用 Docker 来运行 Live777:

::: danger
**需要用 host 模式的网络**
:::

```sh
docker run --name live777-server --rm --network host ghcr.io/binbat/live777-server:latest live777
```

### Install Live777

从 Gtihub 上下载二进制包直接运行

```bash
./live777
```

### Configuration

```bash
cp conf/live777.toml live777.toml

live777 --config live777.toml
```

