# 开发者

Depends:
- [cargo](https://www.rust-lang.org/)
- [nodejs](https://nodejs.org/) Or [bun](https://bun.sh/)

## Release build

```bash
# Build Web UI
npm install
npm run build

# Live777 Core (SFU Server)
cargo build --release

# Live777 Manager
cargo build --release --bin=liveman

# whipinto / whepfrom
cargo build --release --bin=whipinto
cargo build --release --bin=whepfrom
```

如果你需要配置，使用配置文件

```bash
cp conf/live777.toml live777.toml
cp conf/liveman.toml liveman.toml
```

## Log

可以使用 `RUST_LOG` 环境变量来设置日志分模块输出

比如 `live777`, 目前模块日志输出为：

```bash
RUST_LOG=live777=<cfg.log.level>,net4mqtt=<cfg.log.level>,http_log=<cfg.log.level>,webrtc=error",
```

我们可以用这个变量覆盖默认的日志输出方案

```bash
RUST_LOG=live777=error,net4mqtt=debug,webrtc=error",
```

## 以开发模式运行

### WebUI

```bash
npm install

# live777 webui
npm run dev

# liveman webui
npm run dev:liveman
```

### Live777

```bash
cargo run -- -c conf/live777.toml
```

### LiveMan

```bash
cargo run --bin=liveman --features=liveion -- -c conf/liveman.toml
```

### whipinto && whepfrom

```bash
cargo run --bin=whipinto
cargo run --bin=whepfrom
```

So. We support parameter `command`, You can use this:

```bash
cargo run --bin=whipinto -- -i input.sdp -w http://localhost:7777/whip/777 --command \
"ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp"
```

```bash
cargo run --bin=whepfrom -- -o output.sdp -w http://localhost:7777/whep/777 --command \
'ffplay -protocol_whitelist rtp,file,udp -i output.sdp'
```

