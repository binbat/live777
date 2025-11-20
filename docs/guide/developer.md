# For developer

Depends:
- [cargo](https://www.rust-lang.org/)
- [nodejs](https://nodejs.org/)
- [pnpm](https://pnpm.io/)

OptDepends:
- [ffmpeg](https://www.ffmpeg.org/)
- [just](https://github.com/casey/just)
- [cross-rs](https://github.com/cross-rs/cross)
    - `docker` / `containerd` / `podman`
- [nfpm](https://nfpm.goreleaser.com/)

## Binary and source code {#binary}

Binary     | Package    | Comment
---------- | ---------- | -----------
`live777`  | `liveion`  | Core, SFU Server
`liveman`  | `liveman`  | Live777 Cluster Manager
`livecam`  | `livecam`  | Live777 Camera Suit <Badge type="warning" text="experimental" />
`whipinto` | `livetwo`  | rtp, rtsp to whip
`whepfrom` | `livetwo`  | whep ro rtp, rtsp
`livenil`  | `livenil`  | Only at developer, test, demo

## Release build {#release}

```bash
# Build Web UI
pnpm install
pnpm -r build

# Live777 Core (SFU Server)
cargo build --release

# Live777 Manager
cargo build --release --bin=liveman

# whipinto / whepfrom
cargo build --release --bin=whipinto
cargo build --release --bin=whepfrom
```

If you need configuration, you can use

```bash
cp conf/live777.toml live777.toml
cp conf/liveman.toml liveman.toml
```

## Custom log {#log}

Use `RUST_LOG` environment variable for set custom log level

For `live777`, default log set

```bash
RUST_LOG=live777=<cfg.log.level>,net4mqtt=<cfg.log.level>,http_log=<cfg.log.level>,webrtc=error",
```

You can use this for override default log set

```bash
RUST_LOG=live777=error,net4mqtt=debug,webrtc=error",
```

## Run in developer mode {#developer-mode}

### WebUI

```bash
pnpm install

# live777 debugger
pnpm --filter debugger dev

# live777 webui
pnpm --filter liveion dev

# liveman webui
pnpm --filter liveman dev
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

If you want quick up a cluster, you can use this up a cluster for develop environment

up one `liveman` and N `live777`

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

## Use Web browser debug {#browser}

Most browser build-in WebRTC debug tools

| Firefox        | Chrome                       | Edge                       |
| -------------- | ---------------------------- | -------------------------- |
| `about:webrtc` | `chrome://webrtc-internals/` | `edge://webrtc-internals/` |
|                | `chrome://webrtc-logs/`      | `edge://webrtc-logs/`      |

