# NET4MQTT (NET For MQTT)

This is a networking proxy extra module

```
client <---> local <--[MQTT]--> agent <---> server
```

This tool look like [Shadowsocks](https://shadowsocks.org/) or [V2Ray](https://www.v2ray.com/), But network in MQTT

This mode module map:

`NET4MQTT`    | `V2Ray`    | `Shadowsocks`
------------- | -----      | -----------
`agent`       | `freedom`  | `ss-server`
`local-port`  | `dokodemo` | `ss-local::tunnel`
`local-socks` | `socks`    | `ss-local::socks`

## MQTT Topic

```
<prefix>/<agent id>/<local id>/<label>/<protocol>/<src(addr:port)>/<dst(addr:port)>
```

### network input/output

Publish topic example:

```
prefix/agent-0/local-0/i/udp/127.0.0.1:4444/127.0.0.1:4433
prefix/agent-0/local-0/o/udp/127.0.0.1:4444/127.0.0.1:4433
prefix/agent-0/local-0/o/udp/127.0.0.1:4444/-
```

Subscribe topic example:

```
TOPIC: <prefix>/< + | agent id>/< + | local id>/<label>/#

Sub topic example: prefix/+/local-0/i/#
Sub topic example: prefix/agent-0/+/o/#
```

::: warning
Only MQTT QoS: `0`
:::

### online/offline status sync (Option)

```
prefix/agent-0/local-0/v/-
```

- Retain: `true`
- QoS: `1`


### Agent

If No set `dst`, use default `target` as `dst`

### Local-Port
- tcp
- tcp over kcp
- udp

### Local-Socks
- tcp
- tcp over kcp
- cluster internal domain, nslookup to `agent-id`

## net4mqtt-cli

How to use cli(command line interface) tools?

```bash
cargo build --bin=net4mqtt
```

```
Usage: net4mqtt [OPTIONS] <COMMAND>

Commands:
  local-socks  [mode::local], use socks5 proxy. Look like: [shadowsocks::local] or [v2ray::socks]
  local-port   [mode::local], port forwarding. Look like: [shadowsocks::tunnel] or [v2ray::dokodemo]
  agent        [mode::agent]. Look like: [shadowsocks::server] or [v2ray::freedom]
  help         Print this message or the help of the given subcommand(s)

Options:
  -v...          Verbose mode [default: "warn", -v "info", -vv "debug", -vvv "trace"]
  -h, --help     Print help
  -V, --version  Print version
```

1. Up a MQTT broker server

```bash
mosquitto
```

You can use a Monitor MQTT topic messages for debug

```bash
mosquitto_sub -L 'mqtt://localhost:1883/net4mqtt/#' -v
```

### TCP Proxy

TCP/UDP simulation server test

2. up a TCP Server

```bash
nc -l 7777
```

3. up a net4mqtt agent

```bash
net4mqtt -vvv agent --id 0
```

4. up a net4mqtt local

```bash
net4mqtt -vvv local-port --agent-id 0 --id 0
```

5. up a TCP Client

```bash
nc 127.0.0.1 4444
```

For UDP

```bash
nc -l -u 7777
nc -u 127.0.0.1 4444
```

## Integration

- *live777 integration net4mqtt agent*
- *liveman integration net4mqtt local-socks*

![net4mqtt](/net4mqtt.excalidraw.svg)


You can enable `--feature=net4mqtt` to use it.

```bash
cargo build --bin=live777 --features=net4mqtt
cargo build --bin=liveman --features=net4mqtt
```

### Live777

::: tip
live777 binary integration with [net4mqtt](/guide/net4mqtt) `agent`
:::

Enable in `live777.toml`

```toml
[net4mqtt]
mqtt_url = "mqtt://localhost:1883/net4mqtt"
alias = "liveion-0"
```

### Liveman

::: tip
liveman binary integration with [net4mqtt](/guide/net4mqtt) `local-socks`
:::

Enable in `liveman.toml`

```toml
[net4mqtt]
mqtt_url = "mqtt://localhost:1883/net4mqtt"
alias = "liveman-0"
listen = "127.0.0.1:1077"
domain = "net4mqtt.local"
```

