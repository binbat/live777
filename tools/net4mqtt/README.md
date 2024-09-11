# UDP over MQTT

```bash
pushd tools/net4mqtt
```

## TCP/UDP simulate server test

```bash
cargo test --package=net4mqtt -- --nocapture
```

## TCP/UDP simulate server netcat echo

```bash
cargo run --package=net4mqtt -- echo
```

```bash
nc 127.0.0.1 4444
```

```bash
nc -u 127.0.0.1 4444
```

## HTTP3 over MQTT

```bash
cargo run --package=net4mqtt
```

HTTP3 test application

```bash
git clone git@github.com:quinn-rs/quinn.git
```

HTTP3 Server

```bash
cargo run --example server -- --listen="127.0.0.1:4433" ./
```

HTTP3 Client

```bash
cargo run --example client https://127.0.0.1:4444/Cargo.toml --host localhost
```

