---
description: >-
  排查 WHIP/WHEP 桥接故障：认证、HTTP 状态码、ICE 连通性与无媒体。
---

# 排障

`whipinto`、`whepfrom` 和探测工具共用的检查项。先加 `-vv` 运行：WebRTC 和桥接
诊断日志通常能指出失败的阶段。

## 认证与 HTTP 状态

| 现象 | 检查项 |
|------|--------|
| `401 Unauthorized` | 用 `-t <token>` 传入与服务器 `[auth] tokens` 完全一致的 token；不要再手动添加 `Bearer` 前缀。 |
| `403 Forbidden` | 请求已通过认证，但被代理或集群策略拒绝；检查 Live777/liveman 的授权与路由规则。 |
| `409 Conflict` | 该流已被发布者、会话或配置的 source 占用（或达到订阅上限）。停止占用者、更换流名或检查 `strategy.override_publisher`；cascade-pull 发布者不能被替换。 |

`404` 表示端点路径本身不对（`/whip/<stream>` 与 `/whep/<stream>`），或者在禁用
自动创建的策略下流不存在，也可能被反向代理改写了路径。各工具页面列出了自己
期望的端点。

## ICE 超时或断开

仅 host candidate 时，客户端与服务器必须网络直连。远程或 NAT 场景请配置服务器
ICE 地址和 STUN/TURN，并按需传入 `--ice-server`；见
[live777 配置](/zh/guide/live777)。

## 已连接但没有媒体

先确认上游确实在产生媒体：订阅方向要有发布者，发送端正在向 SDP 端口发包，
SDP 编码与实际编码器输出一致。如果工具报告已解码帧数但接收端没有画面，问题在
Live777 下游——检查接收端的 SDP 和防火墙规则。
