# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

这个工具有两种模式：
- `rtp`
- `rtsp as client`

反向的 RTP/RTSP → WHIP 请使用 [whipinto](/zh/guide/whipinto)；完整本机闭环见
[三终端验流步骤](/zh/guide/getting-started#用三个终端验证-rtp-桥接闭环)。

## 获取工具与前置条件

请按[安装指南](/zh/guide/installation)下载软件包或预编译二进制，或在仓库中
运行 `cargo build --release --bin whepfrom` 从源码构建。运行前需要可访问且
已有发布者的 Live777 WHEP 端点。RTP 播放还需要 ffplay 或 VLC 等接收端；
RTSP 输出则需要运行中的 RTSP 服务器。

## Bearer token

如果 `live777.toml` 的 `[auth]` 下配置了 `tokens = ["live777"]`，请用
`-t` 传入相同 token：

```bash
whepfrom -t live777 -w http://127.0.0.1:7777/whep/777 \
  -o rtp://127.0.0.1:9000 --sdp-file output.sdp
```

不要把 token 放进 URL，也不要把真实 token 写入公开的 shell 历史或配置文件。

## 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-o`, `--output` | `sdp://0.0.0.0:8555` | 输出目标：`rtp://` / `rtsp://` / `sdp://` |
| `-w`, `--whep` | 必填 | WHEP 端点 URL |
| `--sdp-file` | `output.sdp` | 写出的 SDP 文件名（RTP 模式） |
| `-t`, `--token` | 无 | WHEP 认证使用的 Bearer token |
| `--command` | 无 | 以子进程方式运行命令 |
| `--cable-url` | 无 | Cable UDP 桥接 URL，用于 DataChannel &lt;-&gt; UDP 转发，例如 `udp://0.0.0.0:9001?host=127.0.0.1&port=9000` |
| `--cable-label` | `-` | DataChannel label，加入服务器上对应的 cable |
| `--ice-server` | 无 | ICE 收集使用的服务器，可重复指定；格式 `<url>[,<username>[,<credential>]]`。默认为空——仅使用 host candidate（WHIP/WHEP 端点会通过 Link 头通告自己的 ICE 服务器） |
| `-v` | `warn` | 提高日志级别（`-v` info，`-vv` debug，`-vvv` trace） |

## RTP

RTP 模式需要 `target` 和 `sdp file`

```bash
whepfrom -o rtp://{target_ip}?video={video_port}&audio={audio_port} -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

```bash
whepfrom -o rtp://localhost?video=9000&audio=9002 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

URL 自带的端口同样可用：视频发往该端口，音频发往端口 + 2
（RTP/AVP 惯例）；`?video=`/`?audio=` 仍可逐轨道覆盖：

```bash
whepfrom -o rtp://localhost:9000 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

主机填组播地址时，whepfrom 就是一个独立的 WHEP → 组播桥——与
live777 的 `rtp://` 输出 target 对应的工具侧形态：

```bash
whepfrom -o rtp://230.1.1.1:1720 -w http://localhost:7777/whep/777 --sdp-file output.sdp
```

组播选项通过 query 参数携带：`?ttl=16`（IPv4 TTL / IPv6 hops，默认
1）和 `?interface=...`（IPv4 组填 IPv4 网卡地址，IPv6 组填网卡序号
或名称）。单播目标配置这两项会被拒绝。

使用 [`ffplay`](/zh/guide/ffmpeg) 来播放

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

使用 [`vlc`](/zh/guide/vlc) 来播放

```bash
vlc output.sdp
```

## 从 live777 拉 RTSP

`rtsp-listen`(whepfrom 作为 RTSP Server）模式已移除。live777 内置了
RTSP server，直接拉流即可：

```bash
ffplay rtsp://localhost:8554/777
```

### Use transport `tcp`

```bash
ffplay rtsp://localhost:8554/777 -rtsp_transport tcp
```

## RTSP Client

`whepfrom` 作为一个客户端，向其他 RTSP Server 推流

```bash
whepfrom -w http://localhost:7777/whep/777 -o rtsp://127.0.0.1:8554/777
```

### Use transport `tcp`

```bash
whepfrom -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/test-rtsp?transport=tcp
```

## 排障

使用 `-vv` 输出 WebRTC 和桥接诊断日志。

| 现象 | 检查项 |
|------|--------|
| `401 Unauthorized` | 用 `-t <token>` 传入与服务器 `[auth] tokens` 完全一致的 token；不要再手动添加 `Bearer` 前缀。 |
| `403 Forbidden` | 请求已通过认证，但被代理或集群策略拒绝；检查 Live777/liveman 的授权与路由规则。 |
| `404 Not Found` | 检查 `/whep/<stream>` URL、流名、端口和反向代理路径，确认发布者已创建该流。 |
| `409 Conflict` | 会话或资源冲突被拒绝；查看服务器日志，清理失效会话并检查订阅上限或流策略后重试。 |
| 已连接但没有媒体 | 确认流有活动发布者并已协商轨道。RTP 模式必须使用 `-o rtp://127.0.0.1:9000 --sdp-file output.sdp`（不能只用裸 SDP 输出），让接收端打开该 SDP，并检查 UDP 防火墙和端口。 |
| ICE 超时或断开 | 仅 host candidate 时，客户端与服务器必须网络直连。远程或 NAT 场景请配置服务器 ICE 地址和 STUN/TURN，并按需传入 `--ice-server`；见 [live777 配置](/zh/guide/live777)。 |

