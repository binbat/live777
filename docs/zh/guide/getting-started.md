# 快速开始

验证 Live777 部署最快的方式是使用 all-in-one 的 `live777-all` 镜像。它
打包了 `live777`、`liveman`、FFmpeg 和 WHIP/WHEP 工具
（`whipinto`、`whepfrom`、`whipsynth`、`whepprobe`），并内置
`smoke-test` 命令，可检查完整的 发布 → 订阅 → 解码 闭环。

## 启动服务器

::: danger 注意
**需要用 host 模式的网络**
:::

```sh
docker run -d --name live777 --rm --network host ghcr.io/binbat/live777-all:latest
```

镜像默认命令是 `live777`。**打开浏览器，输入网址：
`http://localhost:7777/`**

## 运行自检

```sh
docker exec live777 smoke-test
```

`smoke-test` 会通过 WHIP 发布一路合成测试流，再通过 WHEP 订阅回来，并
用 FFmpeg 解码收到的音频和视频——验证包括服务器在内的整条
`WHIP → live777 → WHEP` 媒体通路。

**成功**：命令打印一份 `whepprobe` JSON 报告（编码格式、解码帧数、耗时
等），退出码为 `0`。

**失败**：退出码非零，输出包含诊断信息——是哪一段失败（WHIP 发布、
WHEP 订阅、解码）以及原因。请先看服务器日志（`docker logs live777`）。

## 注意事项：token、远程客户端、ICE

- 如果服务器要求 Bearer token（`live777.toml` 中的 `[auth] tokens`），
  通过 `SMOKE_TOKEN` 环境变量传给自检：

  ```sh
  docker exec -e SMOKE_TOKEN=<token> live777 smoke-test
  ```

- 要检查**远程** live777 而不是容器内自带的那个，用 `SMOKE_SERVER_URL`
  指向它——容器内无需运行服务器：

  ```sh
  docker run --rm --network host -e SMOKE_SERVER_URL=http://<服务器>:7777 \
    ghcr.io/binbat/live777-all:latest smoke-test
  ```

- CI 和排障场景还可以调整 `SMOKE_STREAM`、`SMOKE_ATTEMPTS` 和
  `SMOKE_READY_TIMEOUT`；默认值分别是唯一流名、5 次探测尝试和 30 秒
  服务器就绪超时。

- 自检完全在容器内针对本机服务器运行，因此只验证服务器本身。**其他机
  器上**的客户端还需要 ICE 可用：默认配置没有任何 ICE 服务器（仅
  host candidate，仅限局域网）。面向远程或需要穿越 NAT 的客户端，请
  在 `live777.toml` 中配置 `[[ice_servers]]` / `[webrtc] ice_udp_addrs`
  ——见 [live777 配置指南](/zh/guide/live777)。

## 停止与清理

```sh
docker stop live777
```

`--rm` 参数会自动删除容器。

## standalone 变体

如果还没有运行中的服务器，一条命令即可启动临时服务器、完成检查并全
部清理：

```sh
docker run --rm --network host ghcr.io/binbat/live777-all:latest smoke-test --standalone
```

standalone 模式使用独立的临时配置和仅限 loopback 的 ICE；可通过
`SMOKE_PORT` 修改 HTTP 端口（默认 `7777`）。

## 用三个终端验证 RTP 桥接闭环

要权威验证本机 `FFmpeg → WHIP → WHEP → ffplay` 全链路，请先安装
`live777`、`whipinto`、`whepfrom`、FFmpeg 和 ffplay（见
[安装指南](/zh/guide/installation)）。如果 Docker 服务器已占用 7777 端口，
先停止它，然后在同一工作目录的三个终端分别执行以下命令。

**终端 1——启动 SFU：**

```sh
live777
```

**终端 2——生成 H264 RTP，并用 whipinto 发布：**

```sh
whipinto -i input.sdp -w http://127.0.0.1:7777/whip/777 --command \
  "ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 \
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \
  -f rtp rtp://127.0.0.1:5002 -sdp_file input.sdp"
```

**终端 3——用 whepfrom 订阅，并通过生成的 SDP 播放：**

```sh
whepfrom -w http://127.0.0.1:7777/whep/777 \
  -o rtp://127.0.0.1:9000 --sdp-file output.sdp --command \
  "ffplay -protocol_whitelist rtp,file,udp -i output.sdp"
```

RTP 输出 URL 将视频发送到 9000 端口（有音频时发送到 9002），
`--sdp-file` 生成的 SDP 向 ffplay 描述接收端。结束后在各终端按 Ctrl-C。
认证、其他编码和 RTSP 用法见 [whipinto](/zh/guide/whipinto)、
[whepfrom](/zh/guide/whepfrom) 和 [FFmpeg 指南](/zh/guide/ffmpeg)。

## 自检不覆盖的内容

`smoke-test` 验证的是 `WHIP → live777 → WHEP` 音视频通路及 FFmpeg 解码。
在此之外：

- 要验证**浏览器**渲染，请使用 [WhepWright](/zh/guide/whepwright)，它
  用 Playwright 驱动真实浏览器播放。
- 要桥接 **RTP/RTSP** 源或输出，请使用
  [whipinto](/zh/guide/whipinto) 和 [whepfrom](/zh/guide/whepfrom)——它
  们是协议桥接工具，不是默认验流方案。
