# Live777 Core (liveion)

A Pure Single SFU Server for WebRTC.

Only `WHIP` / `WHEP` protocol supported.

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

- :star: It's working
- :shit: Don't support
- :bulb: I don't know, No testing
- :tv: Note have some problem
  1. Working, But Browser can't player this video, Gstreamer to Gstreamer is working
  2. I don't know why av1 and whep error
  3. [OBS av1 codec can't play](https://github.com/binbat/live777/issues/169)

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

### What is cascade?

![cascade](/cascade.excalidraw.svg)

### Very large cluster

![mash-cascade](/mash-cascade.excalidraw.svg)

live777 Cascade have two mode:
- `cascade-pull`
- `cascade-push`

![live777-cascade](/live777-cascade.excalidraw.svg)

## DataChannel Forward

> NOTE: About `createDataChannel()`
> 1. Live777 Don't support `label`, `createDataChannel(label)` this `label` is not used
> 2. Live777 Don't support `negotiated`, `{ id: 42, negotiated: true }` this don't support

![live777-datachannel](/live777-datachannel.excalidraw.svg)

