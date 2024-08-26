# For developer

Depends:
- [cargo](https://www.rust-lang.org/)
- [nodejs](https://nodejs.org/) Or [bun](https://bun.sh/)

## Release build

```bash
# Build Web UI
npm install
npm run build

# Live777 Core
cargo build --release

# Live777 Manager
cargo build --release --package=liveman

# whipinto / whepfrom
cargo build --release --package=whipinto
cargo build --release --package=whepfrom
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
cargo run --package=liveman --features=liveion -- -c conf/liveman.toml
```

### whipinto && whepfrom

```bash
cargo run --package=whipinto
cargo run --package=whepfrom
```

So. We support parameter `command`, You can use this:

```bash
cargo run --package=whipinto -- -i stream.sdp -w http://localhost:7777/whip/777 --command \
"ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file stream.sdp"
```

So. You can use this

```bash
cat > stream.sdp << EOF
v=0
m=video 5004 RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
EOF
```

```bash
cargo run --package=whepfrom -- -c vp8 -u http://localhost:7777/whep/777 -t 127.0.0.1:5004 --command 'ffplay -protocol_whitelist rtp,file,udp -i stream.sdp'
```

