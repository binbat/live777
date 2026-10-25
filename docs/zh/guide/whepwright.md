# WhepWright

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
| `--diagnostics-dir` | `whepwright-diagnostics` | 每次运行诊断目录的父目录 |
| `--trace` | 关闭 | 失败时采集 Playwright trace；不能与 `--token` 同时使用 |

## JSON 输出

`--output json` 只向 stdout 写入一个 JSON 结果。包括详细 Playwright runner
日志在内的所有日志均写入 stderr，因此 stdout 可以直接通过管道传给 `jq`
或其他程序。播放失败时退出码仍为非零。

结果中的 `artifacts` 对象包含诊断 `directory`、是否已保留（`retained`）、
已采集文件的路径以及 `capture_errors`。诊断采集失败不会覆盖原始播放错误。

```bash
whepwright -w http://localhost:7777/whep/live --output json
```

## 失败诊断

每次调用都会在 `--diagnostics-dir` 下使用独立的 `run-*` 目录。成功运行会
删除该目录，失败运行会保留以下文件：

- `screenshot.png`：失败时的浏览器页面截图；
- `console.log`：浏览器控制台消息、未捕获的页面异常和 runner 消息；
- `sdp-ice.json`：连接状态时间线、ICE candidate 类型计数、选中的 candidate
  pair 与可用时的往返时间（RTT），以及 RTP 摘要；
- `trace.zip`：Playwright trace，仅在使用 `--trace` 时生成。

`sdp-ice.json` 只包含摘要，不包含 offer 或 answer SDP，也不会向该文件写入
认证 token 或完整 SDP。采集失败会列在结果的 `artifacts.capture_errors` 数组中。
Rust runner 超时时会请求 Node 完成采集并关闭浏览器，随后清理残留的 runner
进程；此时保留的诊断可能不完整。

Playwright trace 可能包含 evaluate 参数和网络 Authorization 头，因此
`--trace --token ...` 会在启动浏览器或创建诊断目录之前被拒绝。必须使用
Bearer token 认证时，请不要使用 `--trace`。

为失败运行保留 trace：

```bash
whepwright -w http://localhost:7777/whep/live \
  --output json --trace --diagnostics-dir ./diagnostics
```

## 退出码

- `0`：播放成功（WHEP 已连接且视频已渲染）。
- `1`：播放失败或发生错误。
