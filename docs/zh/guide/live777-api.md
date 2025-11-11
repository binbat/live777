# Live777 HTTP API

## WHIP && WHEP

`POST` `/whip/:streamId`

Response: [201]

`POST` `/whep/:streamId`

Response: [201]

* * *

`PATCH` `/session/:streamId/:sessionId`

Response: [204]

`DELETE` `/session/:streamId/:sessionId`

Response: [204]

## Stream

### 创建一个流

`POST` `/api/streams/:streamId`

`streamId` 需要唯一标识符​​

你可以使用此配置自动创建流​​

```toml
[strategy]
# WHIP auto a stream
auto_create_whip = true
# WHEP auto a stream
auto_create_whep = true
```

Response: [204]

### Get all Stream

`GET` `/api/streams/`

Response: [200]

- `id`: String, `streamId`
- `createdAt`: Int, `timestamp`
- `publish`: `Object(PubSub)`, about publisher
- `subscribe`: `Object(PubSub)`, about subscriber
- `(publish | subscribe).leaveAt`: Int, `timestamp`
- `(publish | subscribe).sessions`: Array, `sessions`
- `(publish | subscribe).sessions.[].id`: String, `sessionId`
- `(publish | subscribe).sessions.[].createdAt`: Int, `timestamp`
- `(publish | subscribe).sessions.[].state`: String, [RTCPeerConnection/connectionState](https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection/connectionState#value)
- `(publish | subscribe).sessions.[].cascade`: Optional(Object(Cascade))
- `(publish | subscribe).sessions.[].cascade.sourceUrl`: Optional(String(URL))
- `(publish | subscribe).sessions.[].cascade.targetUrl`: Optional(String(URL))
- `(publish | subscribe).sessions.[].cascade.sessionUrl`: String(URL)

例如:

```json
[
  {
    "id": "push",
    "createdAt": 1719326206862,
    "publish": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "08c1f2a0a60b0deeb66ee572bd369f80",
          "createdAt": 1719326206947,
          "state": "connected"
        }
      ]
    },
    "subscribe": {
      "leaveAt": 1719326206862,
      "sessions": []
    }
  },
  {
    "id": "pull",
    "createdAt": 1719326203854,
    "publish": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "41b2c52da4fb1eed5a3bff9a9a200d80",
          "createdAt": 1719326205079,
          "state": "connected",
          "cascade": {
            "sourceUrl": "http://localhost:7777/whep/web-0",
            "sessionUrl": "http://localhost:7777/session/web-0/aabc02240abfc7f4800e8d9a6f087808"
          }
        }
      ]
    },
    "subscribe": {
      "leaveAt": 1719326203854,
      "sessions": []
    }
  },
  {
    "id": "web-0",
    "createdAt": 1719326195910,
    "publish": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "0dc47d8da8eb0a64fe40f461f47c2a36",
          "createdAt": 1719326196264,
          "state": "connected"
        }
      ]
    },
    "subscribe": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "aabc02240abfc7f4800e8d9a6f087808",
          "createdAt": 1719326204997,
          "state": "connected"
        },
        {
          "id": "dab1a9e88b2400cfd4bcfb4487588ef3",
          "createdAt": 1719326206798,
          "state": "connected",
          "cascade": {
            "targetUrl": "http://localhost:7777/whip/push",
            "sessionUrl": "http://localhost:7777/session/push/08c1f2a0a60b0deeb66ee572bd369f80"
          }
        },
        {
          "id": "685beee8650b761116b581a4a87ca9b9",
          "createdAt": 1719326228314,
          "state": "connected"
        }
      ]
    }
  }
]
```

### 销毁一个流

`DELETE` `/api/streams/:streamId`

Response: [204]

## ​级联

`POST` `/api/cascade/:streamId`

Request:

```json
{
  "token": "",
  "sourceUrl": "",
  "targetUrl": "",
}
```

- `token`: Option, auth header
- `sourceUrl`: `Option<WHEP url>`. if has, use pull mode
- `targetUrl`: `Option<WHIP url>`. if has, use push mode
- `sourceUrl` and `targetUrl` at the same time can only one

## 录制

### 开始录制流

`POST` `/api/record/:streamId`

开始录制指定的流。流必须处于活跃状态（有发布者）才能开始录制。需要启用 `recorder` 特性。

请求体（可选）：

```json
{
  "base_dir": "optional/path/prefix"
}
```

- `base_dir`（可选）：覆盖默认的目录前缀。如果不设置，录制会使用 `/:streamId/:record_id/`（10 位 Unix 时间戳）作为目录；当单次录制时长达到 `max_recording_seconds` 时，系统会自动以新的时间戳目录继续录制。

响应: [200]

```json
{
  "id": "camera01",
  "record_id": "1718200000",
  "record_dir": "camera01/1718200000",
  "mpd_path": "camera01/1718200000/manifest.mpd"
}
```

当输出路径末尾不是 10 位 Unix 时间戳（例如自定义 `base_dir` 未包含该段）时，`record_id` 会返回为空字符串。

### 录制状态

`GET` `/api/record/:streamId`

响应: [200]

```json
{ "recording": true }
```

### 停止录制

`DELETE` `/api/record/:streamId`

停止指定流的录制。成功时返回 [200]，响应体为空。

参考： [Recorder](recorder)
