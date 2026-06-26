# WhepPlay

`whepwright` 是一个基于浏览器的 WHEP 播放测试工具。它通过 Playwright 启动真实浏览器（Chromium、Firefox 或 WebKit），订阅 WHEP 端点，并验证浏览器 WebRTC 协议栈能否正常接收和渲染视频流。

如果需要使用 FFmpeg 进行快速无头解码，请查看 [`WhepProbe`](whepprobe)。

## 构建

```bash
# 需要 Node.js 和 Playwright
cargo build --bin whepwright --features whepwright
```

## 用法

使用 Chromium 播放 WHEP 端点：

```bash
whepwright -w http://localhost:7777/whep/live
```

使用 Firefox，并设置 60 秒超时：

```bash
whepwright -w http://localhost:7777/whep/live --browser firefox --timeout 60
```

以可视化浏览器窗口运行，方便调试：

```bash
whepwright -w http://localhost:7777/whep/live --headless=false
```

使用本机安装的 Google Chrome 播放 H.265。注意 headless Chromium 不支持
H.265 WebRTC，所以必须同时开启可视化窗口：

```bash
whepwright -w http://localhost:7777/whep/live \
          --browser chromium --channel chrome --headless=false
```

## 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-w`, `--whep` | 必填 | WHEP 端点 URL |
| `-t`, `--token` | 无 | WHIP/WHEP 认证使用的 Bearer token |
| `--browser` | `chromium` | 浏览器类型：`chromium`、`firefox`、`webkit` |
| `--channel` | 无 | 浏览器 channel，例如 `chrome` 或 `msedge`（仅 Chromium） |
| `--headless` | `true` | 是否以无头模式运行浏览器（`true` 或 `false`） |
| `--output` | `human` | 输出格式：`human`、`json` |
| `--timeout` | `30` | 整体超时时间（秒） |

## 退出码

- `0`：播放成功（WHEP 已连接且视频已渲染）。
- `1`：播放失败或发生错误。
