# WhipSynth

`whipsynth` 是一个合成 WHIP 发布器。它通过 `rsmpeg` crate 调用 FFmpeg 在本地生成音视频测试图案，并发布到 WHIP 端点，无需外部媒体源。

## 使用场景

- 快速验证 WHIP 端点是否能正常接收并转发流。
- 对 Live777 实例进行并发发布负载测试。
- 在不连接真实摄像头或 FFmpeg 流水线的情况下复现特定编解码器问题。

## 构建

```bash
# 构建 whipsynth 二进制文件（需要 FFmpeg 开发库）
cargo build --bin whipsynth --features rsmpeg
```

## 用法

发布一路 640x480 的纯 VP8 视频流到 WHIP 端点：

```bash
whipsynth -w http://localhost:7777/whip/live
```

发布 VP8 视频 + Opus 音频：

```bash
whipsynth -w http://localhost:7777/whip/live --acodec opus
```

使用认证 token 发布 H.264，并运行 60 秒后退出：

```bash
whipsynth -w http://localhost:7777/whip/live \
          -t my-token \
          --vcodec h264 \
          --duration 60
```

## 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-w`, `--whip` | 必填 | WHIP 端点 URL |
| `-t`, `--token` | 无 | WHIP 认证使用的 Bearer token |
| `--vcodec` | `vp8` | 视频编码：`vp8`、`vp9`、`h264`、`h265`、`av1` |
| `--acodec` | 无 | 音频编码：`opus`、`g722`（省略表示不发送音频） |
| `--width` | `640` | 视频宽度（像素） |
| `--height` | `480` | 视频高度（像素） |
| `--fps` | `30` | 视频帧率 |
| `--duration` | 无 | 运行指定秒数后退出 |
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE 收集使用的服务器，可重复指定；格式 `<url>[,<username>[,<credential>]]`（空字符串表示禁用 ICE 服务器） |

## 负载测试模式

`whipsynth` 可以同时启动多个并发发布者。以下选项默认在帮助信息中隐藏，主要用于测试：

```bash
whipsynth -w http://localhost:7777/whip/live --count 10 --spawn-interval-ms 200
```

每个会话会使用独立的 URL：负载测试会自动在 URL 最后一段路径上追加索引。
例如 `--count 3` 且基础 URL 为 `/whip/live` 时，三个会话分别发布到
`/whip/live-0`、`/whip/live-1`、`/whip/live-2`。

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `--count` | `1` | 并发 WHIP 会话数 |
| `--spawn-interval-ms` | `100` | 每个会话启动之间的间隔（毫秒） |

## 退出码

- `0`：发布正常结束（到达 duration 或被中断）。
- `1`：发生错误，例如 WHIP 端点拒绝请求或 PeerConnection 失败。
