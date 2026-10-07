# WhepFrom

`WHEP` to `RTP`/`RTSP` tool

这个工具有两种模式：
- `rtp`
- `rtsp as client`

## 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-o`, `--output` | `sdp://0.0.0.0:8555` | 输出目标：`rtp://` / `rtsp://` / `sdp://` |
| `-w`, `--whep` | 必填 | WHEP 端点 URL |
| `--sdp-file` | `output.sdp` | 写出的 SDP 文件名（RTP 模式） |
| `-t`, `--token` | 无 | WHEP 认证使用的 Bearer token |
| `--command` | 无 | 以子进程方式运行命令 |
| `--channel` | 无 | DataChannel &lt;-&gt; UDP 转发 URL，例如 `udp://0.0.0.0:9001?host=127.0.0.1&port=9000` |
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

使用 [`ffplay`](/guide/ffmpeg) 来播放

```bash
ffplay -protocol_whitelist rtp,file,udp -i output.sdp
```

使用 [`vlc`](/guide/vlc) 来播放

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

`whepfrom` 作为一个客户端，忘其他的 RTSP Server 来推流

```bash
whepfrom -w http://localhost:7777/whip/777 -o rtsp://127.0.0.1:8554
```

### Use transport `tcp`

```bash
whepfrom -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/test-rtsp?transport=tcp
```

