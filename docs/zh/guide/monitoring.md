# 监控

Live777 开箱即用地暴露 Prometheus 指标 —— 端点是默认编译进去的，不需要
任何 cargo feature 或配置开关。仓库自带一套现成的 Prometheus + Grafana
监控栈，既可用于本地，也可作为生产部署的起点。

## 指标端点

每个 `live777` 节点都在其 HTTP 端口（默认 `7777`）上提供指标：

```bash
curl http://localhost:7777/metrics
```

| 指标 | 类型 | 说明 |
|------|------|------|
| `live777_stream` | gauge | 流数量 |
| `live777_publish` | gauge | 发布会话数量 |
| `live777_subscribe` | gauge | 订阅会话数量 |
| `live777_reforward` | gauge | 转发（级联）会话数量 |
| `live777_rtp_bytes_total{direction="in\|out"}` | counter | RTP 媒体字节数（线上大小）；`in` = 从发布端接收，`out` = 发送给订阅端 |

每条流、每个会话的码率和累计总量也可以通过流/会话 REST API 的 `stats`
字段获取（并通过 SSE 实时推送到 WebUI 仪表盘），交互式查看时通常比指标
更方便。

::: warning
`live777_rtp_bytes_total{direction="out"}` 只统计订阅会话。静态 RTP/RTSP
输出 target（`rtp://` / `rtsp://` targets）发出的流量在
[issue #474](https://github.com/binbat/live777/issues/474) 修复前不计入
这些计数器，因此一条正以几 Mbit/s 组播的流可能显示出站流量为零。
:::

::: info
`liveman` 没有 Prometheus 端点。监控集群时请直接抓取每个 `live777`
节点。
:::

## Prometheus + Grafana 监控栈

仓库自带一套自包含的监控栈：

- `compose.monitoring.yml` —— Prometheus + Grafana 服务
- `conf/monitoring/prometheus.yml` —— 抓取配置
- `conf/monitoring/grafana/` —— 预置的数据源和 `live777 overview` 仪表盘

在一个已在运行的 live777 旁边启动它：

```bash
# live777 本身可以用任何方式运行 —— cargo run、systemd 或 Docker
cargo run --release --features=webui

docker compose -f compose.monitoring.yml up -d
```

然后打开：

- Grafana：<http://localhost:3000>（默认登录 `admin` / `admin` ——
  本地以外的部署请修改）
- Prometheus：<http://localhost:9090>

默认抓取目标是 `host.docker.internal:7777`，即运行在 Docker 宿主机上的
live777（Linux 下 compose 文件已把 `host.docker.internal` 映射到宿主机
网关）。要监控多个节点 —— 例如本地的 livenil 集群 —— 把它们加入
`conf/monitoring/prometheus.yml`：

```yaml
scrape_configs:
    - job_name: live777-cluster
      targets:
          - host.docker.internal:7777
          - host.docker.internal:7778
          - host.docker.internal:7779
```

Grafana 仪表盘展示流/发布者/订阅者/转发数量、随时间变化的会话数，以及
每个节点的 RTP 码率/吞吐量，每 5 秒刷新一次。

## 配合负载测试

指标要在负载下才有意义。用 [LiveWrk](./livewrk) 给节点加压的同时观察
仪表盘：

```bash
just livewrk-whip 100 60          # 100 路并发 WHIP 发布
just livewrk-whep 100 60 load-0   # 100 路并发 WHEP 订阅
```

要做可复现的基准测试，固定参数（会话数、时长、编解码器、分辨率），记录
`live777_rtp_bytes_total` 速率以及进程的 CPU 和内存占用，再跨构建版本
对比。
