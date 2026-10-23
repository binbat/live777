# Cable（DataChannel 转发）

**cable** 是 live777 流上一条命名的双向 datachannel 消息管道，以 channel
的 `label` 为键。流通过 `[[stream.<name>.cables]]` 声明 cable。

## 架构

cable 由 **SFU 桥接，不是端到端**：live777 终结每个对端的 SCTP 关联并
重新广播消息。内部每条 cable 是一对广播总线——`to_publisher`（发往
发布者）与 `to_subscribers`（发往全体订阅者）——forward 把每个 label
匹配的对端 channel 桥接上去。因此发布者和所有订阅者上同 label 的
channel 经由这条流互相通信。

需要记住的推论：

- **没有端到端的有序/可靠保证**：每条腿是独立的 SCTP 关联、参数独立；
  中间总线在接收方消费慢时会丢弃其份额（记日志并计入 DataChannel 丢弃
  指标）。
- 参数是**按腿**的：发布者在自己腿上发 unordered、订阅者在另一条腿上
  收 ordered 是合法组合。
- cable 流量不依赖媒体：纯 cable 的流（没有任何 track）是合法部署。

## 配置

```toml
[stream.fpv]
[[stream.fpv.cables]]
label = "control"          # cable 标识（channel label），必填
# ordered = false          # 缺省：WebRTC 默认（有序、可靠）
# max_retransmits = 0      # 部分可靠（按重传次数）；与 max_packet_life_time 互斥
# max_packet_life_time = 100
# protocol = "fc-v1"       # 子协议，默认空
# capacity = 1024          # 该 cable 两条总线的广播缓存（条数）
# create = true            # 默认；见“服务器创建”

# DC source 坐广播席位：入向 UDP 包广播给全体订阅者，订阅者的回话发到其 target。
[[stream.fpv.sources]]
[[stream.fpv.sources.cables]]
label = "control"          # 该端点接入哪条 cable
url = "udp://0.0.0.0:8891?host=127.0.0.1&port=8890"

# DC target 坐订阅者席位：发布者的消息发到其 target，入向包发给发布者。
[[stream.fpv.targets]]
[[stream.fpv.targets.cables]]
label = "control"
url = "udp://?host=127.0.0.1&port=8892"  # 纯导出
```

| 字段 | 默认值 | 含义 |
| ----- | ------ | ---- |
| `label` | （必填） | cable 标识：该 label 的对端 channel 加入此 cable。 |
| `ordered` | 缺省（WebRTC 默认：有序） | 服务器创建 channel 的 `ordered`。 |
| `max_retransmits` | 缺省（可靠） | 按重传次数的部分可靠；与 `max_packet_life_time` 互斥。 |
| `max_packet_life_time` | 缺省 | 按重传时长（毫秒）的部分可靠。 |
| `protocol` | `""` | 服务器创建 channel 的子协议。 |
| `capacity` | `1024` | 该 cable 两条总线的广播缓存容量（条数）。 |
| `create` | `true` | 服务器在每个会话上创建此 cable 的 channel；`false`：仅按 label 路由。 |

## label 路由与 default 对

label 命中已声明 cable 的 channel 加入该 cable 的总线；其余 label 的
channel 混入**匿名 default 总线对**——即 cable 之前的行为——所以声明了
cable 的流上现有客户端照常工作。

## 服务器创建（`create = true`）

默认 `create = true` 时，live777 在每个 offer 带 `m=application`
section 的会话上按配置参数创建该 cable 的 channel。（DCEP 由创建方声明
channel 参数；对端只读接受。浏览器还会把参数镜像到自己的出方向，因此在
浏览器对端参数双向生效。）

- **触发时机**：会话首个入向 channel OPEN——此时 SCTP 关联已建立、
  DTLS 角色已解析，服务器的 channel id 不会与客户端撞车。（在
  `Connected` 时创建太早：SCTP 握手还在进行，OPEN 写会被丢弃。）
- **去重**：label 命中服务器创建 cable 的客户端自建 channel 会被关闭；
  客户端必须改用通过 `ondatachannel` 收到的服务器创建的 channel。
- `create = false` 时 cable 退化为纯 label 路由：各端自建 channel、参数
  归各端。

## cable <-> UDP 端点

DC 端点以 `cables` 条目声明在流的 `sources` 和 `targets` 上，每个条目带
`label`（接入哪条 cable）和 `udp://` URL：

- **DC source** 坐广播席位：入向 UDP 包（`udp://<listen>` authority 部分）
  广播给全体订阅者；订阅者的消息发到其 `?host=`/`?port=` target。
- **DC target** 坐订阅者席位：发布者的消息发到其 target；入向包发给
  发布者。

`udp://` 的 authority（本地绑定）与 `host`/`port` 查询对（固定目的地）
各自独立可选，因此可以表达单向端点（纯注入/纯导出）；省略 target 即
reply-to-sender。端点的监听端口随流的整个生命周期持有——provisioned 流的
reset 不会重新绑端口。

## 限制

- 不支持 `negotiated: true`（带外协商）的 channel。
- offer 必须携带 `m=application`（answer 不能新增 m-line）——客户端在
  offer 前创建至少一个 channel 即可获得。
- 对 Rust（rtc 栈）对端，被接受 channel 的反向仍是有序可靠的，直到上游
  accept 镜像修复落地；浏览器对端会正确镜像，所以服务器创建的 cable 在
  浏览器上完全生效。
- 跨 SFU 没有端到端保证：可靠性参数按腿生效，总线溢出丢弃按接收方记
  日志。

## 客户端

`whipinto --cable-url udp://...` 和 `whepfrom --cable-url udp://...` 照旧把
它们的 control channel 桥接到 UDP（`--cable-label` 可指定加入某个 cable
而非默认）；两者也会自动接受服务器创建的 cable channel（桥跟随当前打
开的 channel）。浏览器客户端只需处理 `ondatachannel`：

```js
pc.ondatachannel = (ev) => {
  // ev.channel.label === "control"，参数即服务器配置
  ev.channel.onmessage = (m) => { /* ... */ };
};
```
