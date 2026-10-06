# 宇树 Go2

宇树 Go2 机器狗的相机以组播方式发送 H264/RTP 到 `230.1.1.1:1720`
（15 fps、1280×720，水平视场角 100°、垂直 56°——见宇树
[多媒体服务](https://support.unitree.com/home/zh/developer/Multimedia_Services)
文档）。SDP 文件源可以直接加入该组播组，live777 无需任何
GStreamer/Python 桥接即可接入相机。

## 不经过 live777 验证相机

在连接到机器狗的主机上，用宇树官方管线即可看到画面：

```bash
gst-launch-1.0 udpsrc address=230.1.1.1 port=1720 multicast-iface=eth0 ! \
  queue ! application/x-rtp,media=video,encoding-name=H264 ! \
  rtph264depay ! h264parse ! avdec_h264 ! videoconvert ! autovideosink
```

## live777 配置

把描述该组播流的 SDP 写到 `/etc/live777/unitree-go2.sdp`：

```
v=0
o=- 0 0 IN IP4 0.0.0.0
s=unitree-go2
c=IN IP4 230.1.1.1
t=0 0
m=video 1720 RTP/AVP 96
a=rtpmap:96 H264/90000
```

然后在 `live777.toml` 里配置流：

```toml
[stream.go2-cam]
# 只在有人观看时加入组播组：最后一个观众离开后组成员关系
# （部分机器狗上连 MCU 的编码器）都会停止。
on_demand = true

[[stream.go2-cam.sources]]
url = "/etc/live777/unitree-go2.sdp"
# 主机上连接机器狗的网卡地址（Go2 出厂子网为 192.168.123.0/24）。
# 不配置则由内核选择，只能收到默认路由网卡上的组播流量。
multicast_interface = "192.168.123.11"
```

启动 live777 后观看：

- Web UI：`http://<host>:7777/` —— 第一个观众触发按需启动前，
  流显示 `standby` 徽标。
- 播放器用 WHEP 端点：`http://<host>:7777/whep/go2-cam`

live777 会缓存相机的 SPS/PPS 并在每个关键帧前重新注入，新加入的
订阅者从下一个 IDR 即可解码，无需等待相机自己的内联参数集周期。

::: tip
常开的（非 `on_demand`）组播流应在网卡地址分配完成后再启动。
打包的 systemd 单元已经将 live777 排在 `network-online.target`
之后，且绑定/加入失败会先按退避重试若干次再判定源启动失败。
:::

对于机器狗集群，在每只狗的主机（或与其连通的边缘节点）上这样
配置，前面再挂一个 [liveman](./liveman.md) 集群管理器——各条流
即可通过单一入口级联到浏览器。

## live777 作为组播发送端

对称方向同样支持：流的 `targets` 可以配置 `rtp://` URL，live777
会把媒体以纯 RTP over UDP 发出——主机地址是组播地址时发送到组播组，
否则发送到单播地址。live777 充当组播发送端，角色与 Go2 相机本身
相同。

```toml
[stream.robot-cam]
# on_demand = true  # 与 WHIP target 相同：rtp target 也是 standing demand，
                    # 流有发布者（或本 target）时源启动，最后一个离开后停止。

[[stream.robot-cam.targets]]
url = "rtp://230.1.1.1:1720"
# multicast_interface = "192.168.123.10"  # 出站网卡（可选）
# ttl = 16                                # 跨路由器时调大（默认 1）
# payload_type = 96        # 固定视频 payload type（96-127，动态段）；不配则
                           # 沿用协商值，动态编码为 96。音频始终自动选择。
# sdp_file = "/etc/live777/robot-cam.sdp"  # 发送开始时写出接收端 SDP 文件
                           # （类似 ffmpeg 的 -sdp_file）
```

发送是 media-driven 的，与 WHIP push target 一致：流获得发布者
（`PublishStarted`）时开始发送，发布者离开（`PublishStopped`）时
停止。第一条视频轨道发送到 URL 指定的端口，第一条音频轨道发送到
端口 + 2，每条轨道的 RTCP sender report 发送到其 RTP 端口 + 1
（RTP/AVP 惯例），接收端可据此估计抖动/丢包并对齐唇音同步。视频在
发送前会经过重组包：每个 IDR 前都会内联 SPS/PPS，中途加入的接收端
从下一个关键帧即可解码——与接入侧提供的保证一致。音频则原样直通。

配置 `sdp_file` 后，live777 在每次开始发送时写出接收端 SDP 文件
（发送停止时不删除——发送端空闲期间组地址与端口依然有效）。生成
的文件可以被另一个 live777 直接作为 SDP 文件源消费，从而实现
live777 → live777 的组播级联：

```toml
[[stream.relayed.sources]]
url = "/etc/live777/robot-cam.sdp"
```

单播目标生成的 `c=` 行是 `127.0.0.1` 占位符，接收方使用前需将其
替换为实际地址。

由于输出就是普通的 RTP/AVP，接收端不需要 live777。与之匹配的
SDP（即 `sdp_file` 写出的内容）：

```
v=0
o=- 0 0 IN IP4 127.0.0.1
s=live777-robot-cam
c=IN IP4 230.1.1.1
t=0 0
m=video 1720 RTP/AVP 96
a=rtpmap:96 H264/90000
```

用与相机本身相同的管线即可验证：

```bash
gst-launch-1.0 udpsrc address=230.1.1.1 port=1720 multicast-iface=eth0 ! \
  queue ! application/x-rtp,media=video,encoding-name=H264 ! \
  rtph264depay ! h264parse ! avdec_h264 ! videoconvert ! autovideosink
```

也可以用 SDP 文件接收端指向上面的描述。组播地址和端口可以任意
配置，但直接沿用 Go2 相机使用的 `230.1.1.1:1720` 意味着现有的
宇树图传接收工具无需任何修改——live777 只是替代相机成为组播源。
