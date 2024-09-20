# NET Over MQTT

## TCP/UDP simulation server test

```bash
cargo test --package=net4mqtt -- --nocapture
```

## How to use cli tools?

```
live777 <---> net4mqtt agent <---> mqtt broker <---> net4mqtt local <---> liveman
```

### Up a MQTT broker server

```bash
mosquitto
```

### Monitor MQTT topic messages

```bash
mosquitto_sub -L 'mqtt://localhost:1883/net4mqtt/#' -v
```

### TCP Proxy


```
./net4mqtt -h
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running `target/debug/net4mqtt -h`
Usage: net4mqtt [OPTIONS] <COMMAND>

Commands:
  local  use local proxy
  agent  use agent proxy
  help   Print this message or the help of the given subcommand(s)

Options:
  -v...                        Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
  -b, --broker <BROKER>        Mqtt Broker Address [default: mqtt://localhost:1883]
  -p, --prefix <PREFIX>        Mqtt Topic Prefix [default: net4mqtt]
  -c, --client-id <CLIENT_ID>  Mqtt Client Id [default: -]
  -h, --help                   Print help
  -V, --version                Print version
```


1. up a TCP Server

```bash
nc -l 7777
```

2. up a net4mqtt agent

```bash
net4mqtt -vvv agent --id 0
```

3. up a net4mqtt local

```bash
net4mqtt -vvv local --agent-id 0 --id 0
```

4. up a TCP Client

```bash
nc 127.0.0.1 6666
```

5. For UDP

```bash
nc -l -u 7777
nc -u 127.0.0.1 6666
```

