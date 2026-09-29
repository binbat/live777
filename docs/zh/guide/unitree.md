# 宇树 Go2

宇树 Go2 机器狗的相机以组播方式发送 H264/RTP 到 `230.1.1.1:1720`
(15 fps,1280×720，水平视场角 100°、垂直 56°——见宇树
[多媒体服务](https://support.unitree.com/home/en/developer/Multimedia_Services)
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

把描述该组播流的 SDP 写到 `/etc/live777/unitree-go2.sdp`:

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
# 只在有人观看时加入组播组:最后一个观众离开后组成员关系
# (部分机器狗上连 MCU 的编码器)都会停止。
on_demand = true

[[stream.go2-cam.sources]]
url = "/etc/live777/unitree-go2.sdp"
# 主机上连接机器狗的网卡地址(Go2 出厂子网为 192.168.123.0/24)。
# 不配置则由内核选择,只能收到默认路由网卡上的组播流量。
multicast_interface = "192.168.123.11"
```

启动 live777 后观看：

- Web UI:`http://<host>:7777/` —— 第一个观众触发按需启动前，
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
