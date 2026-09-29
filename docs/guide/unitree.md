# Unitree Go2

The Unitree Go2 robot dog's camera multicasts H264/RTP to `230.1.1.1:1720`
(15 fps, 1280×720, FOV 100°×56° — see Unitree's
[Multimedia Services](https://support.unitree.com/home/en/developer/Multimedia_Services)
documentation). An SDP file source joins that multicast group directly, so
live777 ingests the camera with no GStreamer/Python bridge in between.

## Verify the camera without live777

On a host connected to the dog, Unitree's own pipeline shows the stream:

```bash
gst-launch-1.0 udpsrc address=230.1.1.1 port=1720 multicast-iface=eth0 ! \
  queue ! application/x-rtp,media=video,encoding-name=H264 ! \
  rtph264depay ! h264parse ! avdec_h264 ! videoconvert ! autovideosink
```

## live777 configuration

Write the SDP describing the multicast stream to
`/etc/live777/unitree-go2.sdp`:

```
v=0
o=- 0 0 IN IP4 0.0.0.0
s=unitree-go2
c=IN IP4 230.1.1.1
t=0 0
m=video 1720 RTP/AVP 96
a=rtpmap:96 H264/90000
```

Then point a stream at it in `live777.toml`:

```toml
[stream.go2-cam]
# Join the group only while someone is watching: the membership (and on
# some dogs even the MCU's encoder) stops when the last viewer leaves.
on_demand = true

[[stream.go2-cam.sources]]
url = "/etc/live777/unitree-go2.sdp"
# Address of the host NIC linked to the dog (the Go2 factory subnet is
# 192.168.123.0/24). Unset lets the kernel choose, which only receives
# traffic arriving on the default-route interface.
multicast_interface = "192.168.123.11"
```

Start live777 and watch:

- Web UI: `http://<host>:7777/` — the stream shows a `standby` badge until
  the first viewer triggers the on-demand start.
- WHEP endpoint for players: `http://<host>:7777/whep/go2-cam`

live777 caches the camera's SPS/PPS and re-injects them ahead of every
keyframe, so a joining subscriber decodes from the next IDR instead of
waiting for the camera's inline parameter-set cadence.

::: tip
Always-on (non-`on_demand`) multicast streams should start after the
interface address is assigned. The packaged systemd units already order
live777 after `network-online.target`, and a failed bind/join is retried
with backoff before the source start fails.
:::

For a fleet of dogs, run this on each dog's host (or an edge box with a
link to it) and put a [liveman](./liveman.md) cluster manager in front —
the streams then cascade to browsers through a single entry point.
