# 树莓派

在树莓派上使用**原生硬件管线**部署 Live777 摄像头推流：libcamera 采集 + V4L2 M2M H.264 硬编码，全部在 `live777` 进程内完成。不需要 ffmpeg，不需要额外进程，不做软件编码。

::: warning 不要在树莓派上使用 `ffmpeg + whipinto`
通用的 `ffmpeg -> whipinto -> live777` 路径是用 CPU 软编码（`libx264`）。在树莓派上这会占满所有核心：帧率低、过热降频、延迟不断累积。下面的 `native-rpi` 构建用硬件完成采集和编码——这才是在 ARM 开发板（树莓派、瑞芯微 RK3588、地平线 RDK X5）上推摄像头的正确方式。
:::

已在 Raspberry Pi Zero 2 W（Debian 13，aarch64）+ OV5647 摄像头（v1）上验证。任何 64 位 Raspberry Pi OS 且 libcamera 工作正常（`rpicam-hello` 能出图）即可。

## 第一步：安装 native-rpi 构建

`v0.9.0` 之后的 Release 会发布预编译的树莓派原生构建包——在
[releases 页面](https://github.com/binbat/live777/releases)选择 `live777-<版本>-aarch64-unknown-linux-gnu-rpi.tar.gz`：

```bash
TAG=v0.9.1   # 目标版本；必须晚于 v0.9.0
wget https://github.com/binbat/live777/releases/download/${TAG}/live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
tar xzf live777-${TAG}-aarch64-unknown-linux-gnu-rpi.tar.gz
cd live777-${TAG}-aarch64-unknown-linux-gnu-rpi
```

压缩包内含 `live777` 二进制、`live777.toml` 配置模板和 `live777.service` systemd 单元。

如果你选的 Release 没有 `-rpi` 资产（原生构建是在 `v0.9.0` 之后加入的），改为从源码构建——见 [livehal](./livehal.md#build)，或用 `just rpi-cross-build` 交叉编译。

## 第二步：配置摄像头源

编辑 `live777.toml`——一个预注册流，挂摄像头源和质量档位阶梯（质量档位功能最早随
[#437](https://github.com/binbat/live777/pull/437) 发布）：

```toml
[stream.pi-cam]
[[stream.pi-cam.sources]]

[stream.pi-cam.sources.capture]
backend = "libcamera"
device = "0"
width = 1296        # OV5647 原生 2x2 合并模式：完整视野
height = 972
fps = 30
pixel_format = "yuv420"

[stream.pi-cam.sources.encoder]
backend = "v4l2-m2m"
codec = "h264"
bitrate = 2000000   # 上限——自适应码率（AIMD）只会从这里往下调
profile = "baseline"
level = "4.0"
gop = 60

[stream.pi-cam.sources.output]
payload_type = 96
clock_rate = 90000

# 质量档位阶梯，运行时可切换。带 capture 块的档位会重建管线
# （亚秒级断帧，订阅者保持连接）；纯 encoder 档位原地无缝调整。
# 省略 encoder.bitrate 时按档位几何参数推导（约 0.07 bpp）。
[[stream.pi-cam.sources.tiers]]
name = "std"
capture = { width = 1296, height = 972, fps = 30 }
encoder = { bitrate = 2000000 }
[[stream.pi-cam.sources.tiers]]
name = "maxres"
capture = { width = 1920, height = 1080, fps = 20 }
encoder = { bitrate = 3000000 }
[[stream.pi-cam.sources.tiers]]
name = "lowlatency"
capture = { width = 640, height = 480, fps = 60 }
encoder = { bitrate = 1000000 }
[[stream.pi-cam.sources.tiers]]
name = "lowbw720"
capture = { width = 1280, height = 720, fps = 10 }
encoder = { bitrate = 500000 }
[[stream.pi-cam.sources.tiers]]
name = "lowbwtiny"
capture = { width = 320, height = 240, fps = 10 }
encoder = { bitrate = 150000 }
```

自适应码率默认开启：AIMD 控制器跟随 WHEP 订阅者的 RTCP 反馈实时调整编码器，当前档位码率为上限、最低档为下限。语义详见
[livehal](./livehal.md#adaptive-bitrate)。

## 第三步：运行

```bash
./live777 --config live777.toml
```

或安装自带的 systemd 单元：

```bash
sudo cp live777 /usr/local/bin/
sudo cp live777.toml /etc/live777.toml
sudo cp live777.service /etc/systemd/system/
sudo systemctl enable --now live777
```

## 第四步：播放

浏览器打开 `http://<树莓派IP>:7777/`——Dashboard 里能看到预注册的 `pi-cam` 流，点击即可通过 WHEP 播放。

## 第五步：运行时切换画质

在 Dashboard 的 **Bitrate** 对话框里切换，或调用 API：

```bash
# 切换阶梯档位
curl -X POST http://<树莓派IP>:7777/api/sources/pi-cam/tier \
  -H 'Content-Type: application/json' -d '{"tier": "lowlatency"}'

# 查看当前驱动模式（adaptive/fixed）和编码器码率
curl http://<树莓派IP>:7777/api/sources/pi-cam/bitrate
```

## 故障排查

- **CPU 约 100%、帧率低、延迟持续累积**——你在软编码（ffmpeg/`libx264` 管线）。换用上面的 `native-rpi` 构建；硬件编码只占*一个*核心的约 20–65%（随分辨率变化）。
- **摄像头被占用 / 黑图**——有别的进程占用传感器（`rpicam-hello`、motion 等）。用 `rpicam-hello --timeout 1` 检查。
- **实际帧率低于配置值**——传感器模式会钳制帧率；各模式上限和 60 fps 参考配置见
  [livehal — 树莓派说明](./livehal.md#raspberry-pi-notes)。
