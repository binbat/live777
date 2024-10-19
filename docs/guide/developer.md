# For developer

Depends:
- [cargo](https://www.rust-lang.org/)
- [nodejs](https://nodejs.org/) Or [bun](https://bun.sh/)

## Binary and source code

Binary     | Package    | Comment
---------- | ---------- | -----------
`live777`  | `liveion`  | Core, SFU Server
`liveman`  | `liveman`  | Live777 Cluster Manager
`whipinto` | `livetwo`  | rtp, rtsp to whip
`whepfrom` | `livetwo`  | whep ro rtp, rtsp
`livenil`  | `livenil`  | Only at developer, test, demo

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

If you need configuration, you can use

```bash
cp conf/live777.toml live777.toml
cp conf/liveman.toml liveman.toml
```

## Run in debug mode

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

