# LiveWrk

`livewrk` 是一个面向 WHIP/WHEP 端点的负载测试工具，名字源自 HTTP 压测工具
`wrk`。它可以对 Live777 实例发起大量并发发布或订阅会话，并输出会话、流量和
RTCP 反馈统计。

## 使用场景

- 用数百个并发 WHIP 发布者对 Live777 实例做压力测试。
- 用大量 WHEP 订阅者压测单条流的 SFU 扇出路径。
- 在系统承压的同时持续验证媒体仍然可解码（旋转解码验证）。

## 构建

```bash
# whip 子命令和 WHEP 解码验证需要 FFmpeg 开发库（rsmpeg）
cargo build --bin livewrk --features rsmpeg
```

不带 `rsmpeg` feature 构建时只有 `whep` 子命令可用（且不能用
`--verify-window`）；此时调用 `whip` 子命令会提示如何重新构建。

## 用法

发布 100 路合成流（`load-0` .. `load-99`），运行 60 秒：

```bash
livewrk whip --whip http://localhost:7777/whip/load --sessions 100 --duration 60
```

向一条已发布的流发起 100 个订阅会话：

```bash
livewrk whep --whep http://localhost:7777/whep/load-0 --sessions 100 --duration 60
```

`whip` 子命令会在 URL 最后一段路径上追加 `-N`，因此每个会话发布独立的流。
`whep` 请指向其中一条流（例如 `load-0`）或任意其他已发布的流。

`justfile` 中提供了现成的配方：

```bash
just livewrk-whip 100 60
just livewrk-whep 100 60 load-0
```

## 通用选项

两个子命令共用以下选项：

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `-w`, `--whip` / `--whep` | 必填 | WHIP/WHEP 端点 URL |
| `-t`, `--token` | 无 | 认证使用的 Bearer token |
| `--sessions` | `100` | 并发会话数 |
| `--ramp-ms` | `10` | 每个会话启动之间的间隔（毫秒，爬坡） |
| `--duration` | `60` | 总运行时长（秒），到期后会话停止 |
| `--output` | `human` | 报告格式：`human` 或 `json` |
| `--min-connected` | `0` | 成功所需的最少已连接会话数 |
| `--min-packets` | `0` | 成功所需的最少聚合包数 |
| `--min-bytes` | `0` | 成功所需的最少聚合字节数 |
| `--fail-on-any-session` | `false` | 任一会话失败或被中止时判定运行失败 |
| `-v`, `-vv`, `-vvv` | `warn` | 日志级别：`info`、`debug`、`trace` |

三个阈值都针对运行结束时的聚合报告，而不是每个会话。未设置这些选项时，
只要至少一个会话连接成功，就保留原有的部分失败语义并以成功退出；失败或被
中止的会话会列为警告。全部会话失败始终是错误，正常取消的会话不算失败。

## `whip` 选项

`whip` 子命令发布进程内生成的合成测试图案（与 [WhipSynth](./whipsynth)
相同的引擎），无需外部编码器。

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `--vcodec` | `vp8` | 视频编码：`vp8`、`vp9`、`h264`、`h265`、`av1` |
| `--acodec` | 无 | 音频编码：`opus`、`g722`（省略表示不发送音频） |
| `--width` | `640` | 视频宽度（像素） |
| `--height` | `480` | 视频高度（像素） |
| `--fps` | `30` | 视频帧率 |
| `--ice-server` | 无 | ICE 收集使用的服务器，可重复指定；格式 `<url>[,<username>[,<credential>]]`。默认为空——仅使用 host candidate（WHIP/WHEP 端点会通过 Link 头通告自己的 ICE 服务器） |

## `whep` 选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `--verify-window` | 无 | 启用旋转解码验证（每个窗口的秒数） |
| `--verify-tolerant` | `false` | 只报告验证失败，不让整个运行失败 |
| `--ice-server` | 无 | ICE 收集使用的服务器，可重复指定；格式 `<url>[,<username>[,<credential>]]`。默认为空——仅使用 host candidate（WHIP/WHEP 端点会通过 Link 头通告自己的 ICE 服务器） |

### 旋转解码验证

使用 `--verify-window N` 时，单个验证器每次只解码一个会话，持续 N 秒后轮换到
下一个活跃会话。解码开销与会话数无关，因此即使大规模运行也能验证 SFU 转发的
媒体始终可解码。被停机截断的窗口不计入统计；如果没有任何一个完整窗口，或存在
失败的窗口，运行将以非零退出码结束（除非指定 `--verify-tolerant`）。构建中不
支持解码的编码格式会在验证备注中说明，而不是判为失败。

## 输出

运行结束时 `livewrk` 会打印汇总：

```
══════════════════════════════════════════════
  whep loadtest results
  Sessions: 100 total, 100 connected, 0 failed, 0 cancelled, 0 aborted
  Packets: 152340, bytes: 52428800 (52.43 MB)
  Avg connected duration: 58.9s
══════════════════════════════════════════════
```

媒体写入错误和 RTCP 反馈（NACK/PLI）在非零时显示；`whip` 运行统计发送的包数，
`whep` 运行统计接收的包数。

使用 `--output json` 时，stdout 只包含一个 JSON 报告，日志写入 stderr。schema
第 1 版包含以下顶层字段（成功和失败的运行都会输出相同字段）：

```json
{
  "schema_version": 1,
  "stats": {
    "sessions_total": 2,
    "sessions_connected": 2,
    "sessions_failed": 0,
    "sessions_cancelled": 0,
    "sessions_aborted": 0,
    "total_packets": 152340,
    "total_bytes": 52428800,
    "total_errors": 0,
    "total_nack_count": 0,
    "total_pli_count": 0,
    "total_connected_duration": { "secs": 20, "nanos": 0 }
  },
  "verification": null,
  "failures": [],
  "warnings": [],
  "success": true
}
```

未请求验证或验证结果丢失时，`verification` 为 `null`；否则包含旋转验证器的
窗口、会话、帧、备注和错误统计。`failures` 说明未通过的验收检查；`warnings`
记录被容忍的验证问题、部分会话失败，以及全部会话均正常取消的运行。即使全部
会话失败，也会先输出此报告，再以非零退出码结束。收集开始前发生的运行错误
（例如非法 WHIP URL 或构建不支持解码验证）在 JSON 模式下同样输出此结构：
`success` 为 `false`，`failures` 包含错误，`stats` 和 `verification` 使用默认
空值。命令行参数解析错误发生在确定 JSON 模式之前，仍使用标准 CLI 错误输出。

## 退出码

- `0`：所有已配置的验收检查通过（或第一次 Ctrl-C 触发优雅停机后，最终报告
  通过）。默认允许部分会话失败；验证失败可通过 `--verify-tolerant` 明确容忍。
- `1`：发生运行错误、全部会话失败、未达到已配置的最小阈值、
  `--fail-on-any-session` 检测到失败/中止会话，或解码验证失败且未指定
  `--verify-tolerant`。
- `130`：停机过程中第二次 Ctrl-C 强制立即退出。
