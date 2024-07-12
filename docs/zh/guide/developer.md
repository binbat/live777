# 开发者

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
cargo run --package=liveman --features=liveion -- -c conf/liveman.toml
```

### whipinto && whepfrom

```bash
cargo run --package=whipinto
cargo run --package=whepfrom
```

