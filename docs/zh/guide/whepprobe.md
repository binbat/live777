# WhepProbe

`whepprobe` 是一个类似 `ffprobe` 的 WHEP 端点诊断工具。它订阅 WHEP 流并验证流是否可达、视频是否能通过 `rsmpeg` crate 调用 FFmpeg 正常解码。

如果需要基于真实浏览器的播放验证，请查看 [`WhepWright`](whepwright)。

## 构建

```bash
# 需要 FFmpeg 开发库
cargo build --bin whepprobe --features rsmpeg
```

## 用法

探测一个 WHEP 端点：

```bash
whepprobe -w http://localhost:7777/whep/live
```

指定预期编码和超时时间：

```bash
whepprobe -w http://localhost:7777/whep/live --codec h264 --timeout 60
```

输出 JSON 格式，方便脚本或 CI 使用：

```bash
whepprobe -w http://localhost:7777/whep/live --output json
```

## 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-w`, `--whep` | 必填 | WHEP 端点 URL |
| `-t`, `--token` | 无 | WHIP/WHEP 认证使用的 Bearer token |
| `-v` | `warn` | 提高日志级别（`-v` info，`-vv` debug，`-vvv` trace） |
| `--codec` | 自动检测 | 预期视频编码：`vp8`、`vp9`、`h264`、`h265`、`av1`。`rsmpeg` 后端会从 WHEP 会话自动检测编码，因此该选项仅影响报告结果。 |
| `--sprop-params` | 无 | H.265 sprop 参数（`sprop-vps=...;sprop-sps=...;sprop-pps=...`） |
| `--decode-duration` | `5` | WHEP 连接成功后持续解码的秒数。超过 `10` 的值会被静默截断。 |
| `--output` | `human` | 输出格式：`human`、`json` |
| `--timeout` | `30` | 整体超时时间（秒） |
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE 收集使用的服务器，可重复指定；格式 `<url>[,<username>[,<credential>]]`（空字符串表示禁用 ICE 服务器） |

## 退出码

- `0`：探测成功（WHEP 已连接且视频已解码）。
- `1`：探测失败或发生错误。

## 核心库

探测逻辑位于 `livetwo::probe`，可以被集成测试或其他 Rust 工具复用：

```rust
use cli::Codec;
use livetwo::probe::{ProbeBackend, ProbeConfig};
use livetwo::probe::rsmpeg::RsmpegProbe;

// RsmpegProbe 需要 `livetwo` 启用 `rsmpeg` feature 才能使用。
let config = ProbeConfig {
    whep_url: "http://localhost:7777/whep/live".to_string(),
    video_codec: Some(Codec::Vp8),
    ..Default::default()
};

let result = RsmpegProbe::default().probe(&config).await?;
assert!(result.success);
```
