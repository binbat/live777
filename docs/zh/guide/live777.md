# Live777 Core (liveion)

一个为 WebRTC 而生的 SFU 服务器。

默认支持 `WHIP` / `WHEP` 协议。启用 `rtsp` feature 后，liveion 还可以作为
RTSP 服务器运行：通过 `ANNOUNCE/RECORD` 推流，通过 `DESCRIBE/PLAY` 拉流。

a core SFU server, If you need a single server, use this

## 目前支持的编码 {#codec}

| protocol | video codecs                        | audio codecs   |
| -------- | ----------------------------------- | -------------- |
| `WHIP`   | `AV1`, `VP9`, `VP8`, `H265`, `H264` | `Opus`, `G722` |
| `WHEP`   | `AV1`, `VP9`, `VP8`, `H265`, `H264` | `Opus`, `G722` |
| `RTSP`   | `AV1`, `VP9`, `VP8`, `H265`, `H264` | `Opus`, `G722` |

![live777-apps](/live777-apps.excalidraw.svg)

## 目前客户端的支持情况 {#client}

Application          | `AV1`  | `VP9`  | `VP8`  | `H265` | `H264` | `OPUS` | `G722` |
------------------   | :----: | :----: | :----: | :----: | :----: | :----: | :----: |
Browser `whip`       | :star: | :star: | :star: | :star: | :star: | :star: | :star: |
Browser `whep`       | :star: | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whip`     | :star: | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whep`     | :tv: 2 | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whipinto` | :tv: 1 | :star: | :star: | :star: | :star: | :tv: 1 | :star: |
Gstreamer `whepfrom` | :tv: 2 | :star: | :star: | :star: | :star: | :star: | :star: |
FFmpeg `whipinto`    | :shit: | :star: | :star: | :star: | :star: | :star: | :star: |
FFmpeg `whepfrom`    | :shit: | :star: | :star: | :star: | :star: | :star: | :star: |
VLC `whipinto`       | :shit: | :shit: | :star: | :star: | :star: | :star: | :shit: |
VLC `whepfrom`       | :shit: | :shit: | :star: | :star: | :star: | :star: | :shit: |
OBS Studio `whip`    | :tv: 3 | :shit: | :shit: | :star: | :star: | :star: | :shit: |

- :star: 正常运行
- :shit: 不支持
- :bulb: 未知/未测试
- :tv: 存在问题（需注意）
  1. 正常运行，但浏览器无法播放此视频，Gstreamer 到 Gstreamer 传输正常
  2. 我不知道为什么 av1 和 whep 会出错
  3. [OBS av1 编解码器无法播放](https://github.com/binbat/live777/issues/169)

## 认证

### 关闭认证 {#noauth}

::: danger 注意
默认是关闭认证的
:::

如果没有设置任何关于 `[auth]` 块的内容，会关闭认证

### Bearer token {#token}

静态的 HTTP bearer token 只能是超级管理员权限。
一般用于开发，测试和集群管理

```toml
# WHIP/WHEP auth token
# Headers["Authorization"] = "Bearer {token}"
[auth]
# static JWT token, superadmin, debuggger can use this token
tokens = ["live777"]
```

### JWT(JSON Web Token) {#JWT}

JWT 里面包含了权限信息，可以对单个流的推拉流和管理进行授权

```toml
# WHIP/WHEP auth token
# Headers["Authorization"] = "Bearer {token}"
[auth]
# JSON WEB TOKEN secret
secret = "<jwt_secret>"
```

## Cascade

### 什么是 cascade?

![cascade](/cascade.excalidraw.svg)

### 庞大的集群

![mash-cascade](/mash-cascade.excalidraw.svg)

live777 Cascade 有两种模式：
- `cascade-pull`
- `cascade-push`

![live777-cascade](/live777-cascade.excalidraw.svg)

### 静态 cascade-pull（WHEP 源）

`cascade-pull` 除了调用 API，也可以声明为静态流的输入。构建时启用
`source-whep` feature，然后在预注册流上添加 `whep://` 源即可；它与其他
配置源一样参与完整的生命周期（`on_demand` 启停、断线重连、RTCP 关键帧
反馈）：

```toml
[[stream.cam1.sources]]
url = "whep://edge-0:7777/whep/cam1"
# 需要 Bearer 鉴权时：
# url = "whep://token@edge-0:7777/whep/cam1"
```

当这个源配合 `on_demand = true` 使用时，即使 `on_demand_start_timeout_ms`
更低，live777 也会至少等待 `35000ms` 让上游 WHEP 源就绪。这样冷启动的
上游 WHEP/on-demand 源可以完成 HTTP setup 超时预算并送出第一包媒体。

外出的 WHEP peer 使用服务器自己的 `[[ice_servers]]` 配置收集 ICE
候选，并使用 `[webrtc] ice_udp_addrs` 绑定的 UDP 端口（不再有硬编码的
STUN 服务器）。

链式 on-demand 拉流默认支持一跳：该源的 WHEP HTTP 请求最长等待
`40000ms` 应答，可以覆盖自身最多挂起 `35000ms` 的上游（on-demand WHEP
源）。更深的链路需要按部署对齐各级预算。停止该源（例如 `on_demand`
流的最后一个订阅者离开）可能需要等待一个在途的 WHEP HTTP 请求结束，
上限同为 `40000ms`。

## DataChannel 转发

> NOTE: 关于 `createDataChannel()`
> 1. Live777 不支持 `label`, `createDataChannel(label)` 不使用 `label`
> 2. Live777 不支持 `negotiated`, 不支持 `{ id: 42, negotiated: true }`

![live777-datachannel](/live777-datachannel.excalidraw.svg)

## RTSP 服务器

使用 `rtsp` feature 编译，即可在 WHIP/WHEP 之外暴露 RTSP 服务：

```bash
cargo build --release --bin live777 --features rtsp
```

在 `live777.toml` 中配置监听 URL。当 URL 中包含凭据时，自动启用 Digest 认证：

```toml
[rtsp]
# 无认证：
listen = "0.0.0.0:8554"
# 启用 Digest 认证：
# listen = "rtsp://admin:secret@0.0.0.0:8554"
```

同一个端口同时处理推流和拉流：

- 推流地址：`rtsp://host:8554/{stream_id}`（`ANNOUNCE` + `RECORD`）
- 拉流地址：`rtsp://host:8554/{stream_id}`（`DESCRIBE` + `PLAY`）

同时支持 UDP 和 TCP（`RTP/AVP/TCP`）传输。URL 的第一段路径作为 liveion 的流标识符。

## 流钩子（Stream Hooks）

Live777 可以在流生命周期事件上执行外部脚本：流的创建/删除，以及推流
开始/停止（WHIP/cascade 推流端挂载，或配置源启停）。典型用途是为
live777 无法直接驱动的设备做按需激活：真正需要媒体时由 hook 启动采集
设备 / 硬件编码器，最后一个消费者离开时再把它关掉以节省资源。

> 内置替代方案：配置的源（`[[stream.<name>.sources]]`）支持 `on_demand = true`，
> 可随订阅者有无自动启停摄像头或 RTSP 拉流——无需编写脚本。参见
> [livehal — 预注册流与按需源](./livehal#预注册流与按需源on-demand)。

> 哪些事件代表"有人在看"？对**预注册**流（任何 `[stream.<name>]` 条目），
> `stream-created` 在启动时触发一次，无法用于电源控制——请改用
> `publish-started` / `publish-stopped`。配置源的启停以会话 ID
> `virtual-source` 上报为推流事件。对**动态**流（`auto_create_whep`），
> `stream-created` 仍与首次使用同时发生。

```toml
# 全局 hook，对所有流生效
[hooks]
timeout_ms = 5000    # 单个脚本超时（毫秒），0 表示不限制
on_error = "stop"    # "stop"：脚本失败后跳过本事件剩余 hook；
                     # "continue"：失败后仍继续执行
on_stream_created = ["/etc/live777/hooks/notify.sh"]
on_stream_deleted = ["/etc/live777/hooks/notify.sh", "/etc/live777/hooks/cleanup.sh"]
on_publish_started = ["/etc/live777/hooks/camera-power.sh"]
on_publish_stopped = ["/etc/live777/hooks/camera-power.sh"]

# 每流 hook，在全局 hook 之后执行
[stream.cam1.hooks]
on_stream_created = ["/etc/live777/hooks/cam1-created.sh"]
on_stream_deleted = ["/etc/live777/hooks/cam1-deleted.sh"]
on_publish_started = ["/etc/live777/hooks/cam1-power-on.sh"]
on_publish_stopped = ["/etc/live777/hooks/cam1-power-off.sh"]
```

执行保证：

- 同一事件的 hook 按顺序串行执行：先全局、后每流，各自按配置顺序。
- 所有事件共用一个 FIFO 队列处理——前一事件的全部 hook 执行完毕后，后
  一事件的第一个 hook 才开始，因此同一条流的 `stream-created` hook 一定先于
  它的 `stream-deleted` hook 执行完。
- 脚本失败（非零退出、启动失败、超时被杀）只会被记录并按 `on_error` 处
  理，不会影响后续事件，也不会影响服务器本身。

脚本以直接执行方式启动（不经过 shell），事件元数据同时通过 argv 和环境
变量传入：

| argv              | 环境变量          | 取值                                                                                     |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------- |
| `$1`              | `LIVE777_EVENT`   | `stream-created` / `stream-deleted` / `publish-started` / `publish-stopped`              |
| `$2`              | `LIVE777_STREAM`  | 流名                                                                                     |
| `$3`（仅删除/停止时）| `LIVE777_REASON` | stream-deleted:`api-deleted` / `publish-leave-timeout` / `subscribe-leave-timeout` / `orphaned` / `reset`;publish-stopped:`peer-closed` / `api-deleted` / `idle-timeout` |
| —                 | `LIVE777_SESSION` | （仅推流事件）推流会话 ID；配置源为 `virtual-source`                                       |

注意事项：

- 脚本应在发起工作后尽快返回（例如把编码器放到后台启动）。脚本阻塞多久，
  整个 hook 队列就阻塞多久。
- 脚本要幂等：推流端自身死亡（`publish-leave-timeout`）也会触发
  `stream-deleted` hook，停止脚本必须能容忍设备已经停止的状态。
- 做按需源时，配合 `[strategy] auto_create_whep = true` 和较大的
  `auto_delete_whep`（如 `30000`），避免观众短暂掉线导致硬件反复启停。
  每流覆盖配置在 `[stream.<name>.strategy]` 下。配置了 `on_demand = true`
  的流已通过 `on_demand_close_after_ms` 自带防抖。
- 服务器关闭时不会触发 hook（此时不产生 `stream-deleted` 事件）。
