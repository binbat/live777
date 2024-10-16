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

### NET4MQTT

```bash
cargo run --features=net4mqtt
cargo run --features=net4mqtt --bin=liveman
```


Enable in `live777.toml`

::: tip
live777 binary build with [net4mqtt](/zh/guide/net4mqtt) agent
:::

```toml
[net4mqtt]
mqtt_url = "mqtt://localhost:1883/net4mqtt"
alias = "liveion-0"
```

Enable in `liveman.toml`

::: tip
liveman binary build with [net4mqtt](/zh/guide/net4mqtt) local sock mode
:::

```toml
[net4mqtt]
mqtt_url = "mqtt://localhost:1883/net4mqtt"
alias = "liveman-0"
listen = "127.0.0.1:1077"
domain = "net4mqtt.local"
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

