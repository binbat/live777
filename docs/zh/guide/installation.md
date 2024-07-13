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

## Cargo

```bash
cargo install --git http://github.com/binbat/live777 whipinto
cargo install --git http://github.com/binbat/live777 whepfrom
```

## Windows

**Winget**

```bash
winget install live777
winget install whipinto
winget install whepfrom
```

