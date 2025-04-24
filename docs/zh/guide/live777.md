# Live777 Core (liveion)

一个为 WebRTC 而生的 SFU 服务器。

仅支持 `WHIP` / `WHEP` 协议.

a core SFU server, If you need a single server, use this

## 目前支持的编码 {#codec}

| protocol | video codecs                | audio codecs   |
| -------- | --------------------------- | -------------- |
| `WHIP`   | `AV1`, `VP9`, `VP8`, `H264` | `Opus`, `G722` |
| `WHEP`   | `AV1`, `VP9`, `VP8`, `H264` | `Opus`, `G722` |

![live777-apps](/live777-apps.excalidraw.svg)

## 目前客户端的支持情况 {#client}

Application          | `AV1`  | `VP9`  | `VP8`  | `H264` | `OPUS` | `G722` |
------------------   | :----: | :----: | :----: | :----: | :----: | :----: |
Browser `whip`       | :star: | :star: | :star: | :star: | :star: | :star: |
Browser `whep`       | :star: | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whip`     | :tv: 1 | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whep`     | :tv: 2 | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whipinto` | :tv: 1 | :star: | :star: | :star: | :star: | :star: |
Gstreamer `whepfrom` | :tv: 2 | :star: | :star: | :star: | :star: | :star: |
FFmpeg `whipinto`    | :shit: | :star: | :star: | :star: | :star: | :star: |
FFmpeg `whepfrom`    | :shit: | :star: | :star: | :star: | :star: | :star: |
VLC `whipinto`       | :shit: | :shit: | :star: | :star: | :star: | :shit: |
VLC `whepfrom`       | :shit: | :shit: | :star: | :star: | :star: | :shit: |
OBS Studio `whip`    | :tv: 3 | :shit: | :shit: | :star: | :star: | :shit: |

- :star: 正常运行
- :shit: 不支持
- :bulb: 未知/未测试​​
- :tv: 存在问题（需注意）
  1. 正常运行，但浏览器无法播放此视频，Gstreamer 到 Gstreamer 传输正常
  2. 我不知道为什么 av1 和 whep 会出错
  3. [OBS av1 编解码器无法播放](https://github.com/binbat/live777/issues/169)

## 认证

### 关闭认证 {#noauth}

::: danger 注意
默认是关闭认证的
:::

如果没有设置任何关于 `[auth]` 块的内容，会关闭认证

### Bearer token {#token}

静态的 HTTP bearer token 只能是超级管理员权限。
一般用于开发，测试和集群管理

```toml
# WHIP/WHEP auth token
# Headers["Authorization"] = "Bearer {token}"
[auth]
# static JWT token, superadmin, debuggger can use this token
tokens = ["live777"]
```

### JWT(JSON Web Token) {#JWT}

JWT 里面包含了权限信息，可以对单个流的推拉流和管理进行授权

```toml
# WHIP/WHEP auth token
# Headers["Authorization"] = "Bearer {token}"
[auth]
# JSON WEB TOKEN secret
secret = "<jwt_secret>"
```

## Cascade

### 什么是 cascade?

![cascade](/cascade.excalidraw.svg)

### 庞大的集群

![mash-cascade](/mash-cascade.excalidraw.svg)

live777 Cascade 有两种模式：
- `cascade-pull`
- `cascade-push`

![live777-cascade](/live777-cascade.excalidraw.svg)

## DataChannel 转发

> NOTE: 关于 `createDataChannel()`
> 1. Live777 不支持 `label`, `createDataChannel(label)` 不使用 `label` 
> 2. Live777 不支持 `negotiated`, 不支持 `{ id: 42, negotiated: true }` 

![live777-datachannel](/live777-datachannel.excalidraw.svg)

