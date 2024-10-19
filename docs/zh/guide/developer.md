# 开发者

Depends:
- [cargo](https://www.rust-lang.org/)
- [nodejs](https://nodejs.org/) Or [bun](https://bun.sh/)

## 二进制包和源码对应关系

Binary     | Package    | Comment
---------- | ---------- | -----------
`live777`  | `liveion`  | 核心服务 SFU Server
`liveman`  | `liveman`  | Live777 集群控制器
`whipinto` | `livetwo`  | rtp, rtsp to whip
`whepfrom` | `livetwo`  | whep ro rtp, rtsp
`livenil`  | `livenil`  | 集群启动器，主要用在开发和测试环境

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
cargo run --bin=liveman -- -c conf/liveman.toml
```

### LiveNil

如果你想开发或测试集群的一些功能，很明显手动依次启动不是一个明智的选择

可以使用这个工具批量启动一个 `liveman` 和 N 个 `live777` 实例

```bash
cargo run --bin=livenil -- -c conf/livenil
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

