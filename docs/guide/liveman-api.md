# LiveMan HTTP API

Live777 Cluster Manager

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

Recording and playback related APIs (proxy and index listing)

### List Streams with Recording Index

`GET` `/api/playback`

Response: [200] `application/json`
```json
["camera01", "roomA", "web-0001"]
```

### List Index by Stream

`GET` `/api/playback/{stream}`

Response: [200] `application/json`
```json
[
  { "year": 2025, "month": 7, "day": 24, "mpd_path": "camera01/2025/07/24/manifest.mpd" }
]
```

### Get Segment File via Proxy

`GET` `/api/record/object/{path}`

- `path`: URL-encoded storage path of the recorded object (e.g. `camera01/2025/07/24/manifest.mpd`)

Response: [200] Binary media data, content-type inferred by extension (e.g. `application/dash+xml` for `.mpd`, `video/mp4` for `.m4s`/`.mp4`).


Response: [200] `application/dash+xml`
```xml
<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" ...>
  <!-- MPEG-DASH manifest content -->
</MPD>
```

### Get Segment File

Proxy access to recorded segment files.

`GET` `/api/record/object/{path}`

Path parameter: Storage path of the segment file (URL encoded)

Response: [200] Binary media data or [302] redirect to storage URL

## Node

`GET` `/api/nodes/`

Response: [200]

- `alias`: String, Alias must be unique
- `url`: String, Node API URL
- `status`: StringEnum("running" | "stopped"), Node contact health: the latest poll result for `poll` nodes (any HTTP response counts as reachable), the SSE connection state for `sse` nodes, and the discovery presence for net4mqtt nodes
- `duration`: String, Round-trip of the latest poll (e.g. `"12ms"`); `"-"` for nodes without polls (SSE / net4mqtt)

For Example:

```json
[
  {
    "alias": "buildin-0",
    "url": "http://127.0.0.1:55581",
    "status": "running",
    "duration": "3ms"
  },
  {
    "alias": "buildin-1",
    "url": "http://127.0.0.1:55582",
    "status": "running",
    "duration": "5ms"
  },
  {
    "alias": "buildin-2",
    "url": "http://127.0.0.1:55583",
    "status": "stopped",
    "duration": "-"
  }
]
```

## Source

Source management is proxied to a named node (`{alias}`): liveman rewrites each request to the same path on that node and forwards it with the node's bearer token injected, so cluster clients manage per-node sources through liveman only. Request/response bodies are the node's liveion source API — see [live777 API / Source](/guide/live777-api#source). An unknown alias fails with [503].

`GET` `/api/sources/{alias}`

List the node's sources.

`POST` `/api/sources/{alias}/{stream}`

Create a source. On success liveman eagerly registers the stream→node mapping so WHEP routing sees it before the next node snapshot.

`GET` `/api/sources/{alias}/{stream}`

Get source info.

`DELETE` `/api/sources/{alias}/{stream}`

Delete the source.

`GET` `/api/sources/{alias}/{stream}/state`

Get the source runtime state.

`GET` `/api/sources/{alias}/{stream}/bitrate`

Get the encoder bitrate telemetry.

`GET` `/api/sources/{alias}/{stream}/tier`

Get the source quality tiers.

`POST` `/api/sources/{alias}/{stream}/tier`

Apply a source quality tier.

## Stream

### Get all Stream

**This API will merge all nodes streams**

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

For Example:

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

### Get a Stream Details

**This API will return a stream in all nodes**

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

