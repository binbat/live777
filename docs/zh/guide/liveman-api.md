# LiveMan HTTP API

Live777 集群管理器

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

## Recording & Playback

录制和回放相关API（需要启用 `recorder` 特性）

### 分片元数据上报

`POST` `/api/segments/report`

**请求体**:
```json
{
  "node_alias": "live777-node-001",
  "stream": "camera01",
  "segments": [{
    "start_ts": 1721827200000000,
    "end_ts": 1721827201000000,
    "duration_ms": 1000,
    "path": "camera01/2024/01/01/segment_00042.m4s",
    "is_keyframe": true
  }]
}
```

**响应**: [200]
```json
{
  "success": true,
  "message": "Segments processed successfully",
  "processed_count": 1
}
```

### 录制流列表

`GET` `/api/record/streams`

**响应**: [200]
```json
{
  "streams": ["camera01", "camera02", "meeting-room"]
}
```

### 时间轴查询

`GET` `/api/record/{stream}/timeline?start_ts=1721827200000000&end_ts=1721827300000000&limit=100&offset=0`

**查询参数**:
- `start_ts` (可选): 开始时间戳，以微秒为单位
- `end_ts` (可选): 结束时间戳，以微秒为单位
- `limit` (可选): 返回的最大分片数量
- `offset` (可选): 跳过的分片数量

**响应**: [200]
```json
{
  "stream": "camera01",
  "total_count": 150,
  "segments": [{
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "start_ts": 1721827200000000,
    "end_ts": 1721827201000000,
    "duration_ms": 1000,
    "path": "camera01/2024/01/01/segment_00042.m4s", 
    "is_keyframe": true,
    "created_at": "2024-01-01T12:00:00Z"
  }]
}
```

### MPEG-DASH 清单

`GET` `/api/record/{stream}/mpd?start_ts=1721827200000000&end_ts=1721827300000000`

**查询参数**:
- `start_ts`: 开始时间戳（可选）
- `end_ts`: 结束时间戳（可选）

**响应**: [200] `application/dash+xml`
```xml
<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" ...>
  <!-- MPEG-DASH manifest content -->
</MPD>
```

### 获取分片文件

代理访问录制的分片文件。

`GET` `/api/record/object/{path}`

路径参数: 分片文件的存储路径（URL编码）

**响应**: [200] 二进制媒体数据 或 [302] 重定向到存储URL

## Node

`GET` `/api/nodes/`

Response: [200]

- `alias`: String, 别名必须唯一
- `url`: String, 节点 API 的 URL 地址
- `pub_max`: Int16, 最大支持推流数
- `sub_max`: Int16, 最大支持订阅数
- `status`: StringEnum("running" | "stopped"), 节点状态

例如:

```json
[
  {
    "alias": "buildin-0",
    "url": "http://127.0.0.1:55581",
    "pub_max": 65535,
    "sub_max": 1,
    "status": "running"
  },
  {
    "alias": "buildin-1",
    "url": "http://127.0.0.1:55582",
    "pub_max": 65535,
    "sub_max": 1,
    "status": "running"
  },
  {
    "alias": "buildin-2",
    "url": "http://127.0.0.1:55583",
    "pub_max": 65535,
    "sub_max": 1,
    "status": "running"
  }
]
```

## Stream

### 获取所有流

**此API将合并所有节点的流**

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
- `(publish | subscribe).sessions.[].cascade`: Optional(Object(Cascade)

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
            "src": "http://localhost:7777/whep/web-0",
            "resource": "http://localhost:7777/session/web-0/aabc02240abfc7f4800e8d9a6f087808"
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
            "dst": "http://localhost:7777/whip/push",
            "resource": "http://localhost:7777/session/push/08c1f2a0a60b0deeb66ee572bd369f80"
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

### 获取流详情

**此API将返回指定流在所有节点上的信息**

`GET` `/api/streams/:streamId`

Response: [200]

```json
{
  "buildin-1": {
    "id": "web-0",
    "createdAt": 1719415906241,
    "publish": {
      "leaveAt": 0,
      "sessions": []
    },
    "subscribe": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "04eaae154975b61d62fc2e81b2b0862f",
          "createdAt": 1719415906274,
          "state": "connected"
        }
      ]
    }
  },
  "buildin-0": {
    "id": "web-0",
    "createdAt": 1719415876416,
    "publish": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "6ea2c116b93dde47032c7ea19349dc78",
          "createdAt": 1719415876510,
          "state": "connected"
        }
      ]
    },
    "subscribe": {
      "leaveAt": 0,
      "sessions": [
        {
          "id": "369227db507bf2addbb55313e0eb99a0",
          "createdAt": 1719415885569,
          "state": "connected"
        }
      ]
    }
  }
}
```

