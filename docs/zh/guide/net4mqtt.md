# NET4MQTT (NET For MQTT)

MQTT 网络代理扩展

```
client <---> local <--[MQTT]--> agent <---> server
```

这个工具就像 [Shadowsocks](https://shadowsocks.org/) 或 [V2Ray](https://www.v2ray.com/) 一样，但网络是用 MQTT 来完成的

如果你熟悉 V2Ray 或者 Shadowsocks，这里有个对照表，可以很容理解这里面的功能：

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

​发布主题示例：​​

```
prefix/agent-0/local-0/i/udp/127.0.0.1:4444/127.0.0.1:4433
prefix/agent-0/local-0/o/udp/127.0.0.1:4444/127.0.0.1:4433
prefix/agent-0/local-0/o/udp/127.0.0.1:4444/-
```

​订阅主题示例：​​

```
TOPIC: <prefix>/< + | agent id>/< + | local id>/<label>/#

Sub topic example: prefix/+/local-0/i/#
Sub topic example: prefix/agent-0/+/o/#
```

::: warning
仅支持 MQTT QoS: `0`
:::

### 在线/离线状态同步（可选配置）​​

```
prefix/agent-0/local-0/v/-
```

- Retain: `true`
- QoS: `1`


### Agent

​若未设置 `dst`，则默认使用 `target` 作为 `dst`

### Local-Port
- tcp
- tcp over kcp
- udp

### Local-Socks
- tcp
- tcp over kcp
- cluster internal domain, nslookup to `agent-id`

## net4mqtt-cli

我们提供了一个独立的命令行工具，可以独立使用或是用来 Debug 产品环境

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

1. 启动一个 MQTT 代理服务器​​

```bash
mosquitto
```

​你可以通过监听 MQTT 主题消息来进行调试​​

```bash
mosquitto_sub -L 'mqtt://localhost:1883/net4mqtt/#' -v
```

### TCP Proxy

​TCP/UDP 模拟服务器测试​​

2. ​启动 TCP 服务器​​

```bash
nc -l 7777
```

3. ​启动 net4mqtt 代理服务​​

```bash
net4mqtt -vvv agent --id 0
```

4. ​启动 net4mqtt 本地服务​​

```bash
net4mqtt -vvv local-port --agent-id 0 --id 0
```

5. ​启动 TCP 客户端​​

```bash
nc 127.0.0.1 4444
```

针对 UDP 协议

```bash
nc -l -u 7777
nc -u 127.0.0.1 4444
```

## Integration

- *​live777 集成 net4mqtt 代理​​*
- *liveman 集成 net4mqtt 本地 socks 服务​​*

![net4mqtt](/net4mqtt.excalidraw.svg)


​您可通过启用 `--feature=net4mqtt` 参数使用该功能​​.

```bash
cargo build --bin=live777 --features=net4mqtt
cargo build --bin=liveman --features=net4mqtt
```

### Live777

::: tip 注意
live777 会集成 [net4mqtt](/zh/guide/net4mqtt) `agent`
:::

在 `live777.toml` 中启用​​

```toml
[net4mqtt]
mqtt_url = "mqtt://localhost:1883/net4mqtt"
alias = "liveion-0"
```

### Liveman

::: tip 注意
liveman 会集成 [net4mqtt](/zh/guide/net4mqtt) `local-socks`
:::

在 `liveman.toml` 中启用​​

```toml
[net4mqtt]
mqtt_url = "mqtt://localhost:1883/net4mqtt"
alias = "liveman-0"
listen = "127.0.0.1:1077"
domain = "net4mqtt.local"
```

