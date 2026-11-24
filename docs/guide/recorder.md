# Recorder

The Recorder in liveion is an optional feature that automatically records live streams into MP4 fragments and saves them to storage. The `recorder` feature must be enabled at compile time.

## Supported Codecs {#codec}

| container        | video codecs         | audio codecs |
| ---------------- | -------------------- | ------------ |
| `Fragmented MP4` | `H264`, `H265`, `VP9`, `AV1` | `Opus`       |

**Recorder does not support the `VP8` codec because `VP8` requires a `WebM` container.** Starting a recording on a stream whose video codec is unsupported fails fast with `400 Bad Request` instead of silently recording without video.

## Liveman Integration {#liveman}

Integrates with [Liveman](/guide/liveman) for centralized playback and proxy access:

- Start recording returns the storage metadata (`record_id`, `record_dir`, and `mpd_path`). `record_id` comes from the 10-digit Unix timestamp in the output path, falling back to the recording start timestamp when the path has none — it is never empty.
- Liveman stores `record_id`/`record_dir` to keep the catalog in sync with storage
- Clients can stream the manifest via `mpd_path`, and Liveman can proxy objects with `GET /api/record/object/{path}`

### Configuration

```toml
[recorder]
# Optional: Node alias to identify this Live777 instance in the cluster
node_alias = "live777-node-001"
```

::: tip
The node_alias is optional but recommended for multi-node deployments to help Liveman identify the source of recording metadata.
:::

## Configuration {#config}

Configure recording parameters in `live777.toml`:

```toml
[recorder]
# Stream name patterns for auto-recording, supports wildcards (default: [])
auto_streams = ["*"]              # Record all streams
# auto_streams = ["room1", "web-*"]  # Record specific streams

# Maximum duration (seconds) for a single recording session before rotation (default: 86_400)
max_recording_seconds = 86_400

# Optional: Node alias for multi-node deployments
node_alias = "live777-node-001"

# Storage backend configuration (default: local filesystem)
[recorder.storage]
type = "fs"
root = "./storage"
```

### Configuration Options

#### Basic Options

- `auto_streams`: Stream name patterns for auto-recording, supports wildcards (default: `[]`)
- `max_recording_seconds`: Maximum duration (seconds) for a single recording session before rotation (default: `86400`, set to `0` to disable auto-rotation)
- `node_alias`: Optional node identifier for multi-node deployments (default: not set)

#### Storage Options

**Filesystem Backend (default):**

- `type`: `"fs"`
- `root`: Root directory for storing recordings (default: `"./storage"`)

::: warning
The filesystem backend supports basic recording only. The [async upload queue](#async-upload) feature requires S3.
:::

**S3 Backend:**

- `type`: `"s3"`
- `bucket`: S3 bucket name (required)
- `root`: Root path within bucket (default: `"/"`)
- `region`: AWS region (optional, auto-detected from environment if not set)
- `endpoint`: Custom endpoint URL for S3-compatible services (optional)
- `access_key_id`: AWS access key ID (optional, can be loaded from environment)
- `secret_access_key`: AWS secret access key (optional, can be loaded from environment)
- `session_token`: Session token for temporary credentials (optional)
- `disable_config_load`: Set to `true` to disable automatic credential loading from environment/config files (default: `false`)
- `enable_virtual_host_style`: Enable virtual-hosted-style requests, e.g., `bucket.endpoint.com` instead of `endpoint.com/bucket` (default: `false`)

## Storage Backend {#storage}

### Local Filesystem (default)

```toml
[recorder.storage]
type = "fs"
root = "./storage"
```

::: info
The filesystem backend supports basic recording (write segments, update MPD). It does not support presigned URLs or the async upload queue.
:::

### AWS S3

Using IAM role (recommended for EC2/ECS):
```toml
[recorder.storage]
type = "s3"
bucket = "my-live777-bucket"
root = "/recordings"
region = "us-east-1"
```

Using explicit credentials:
```toml
[recorder.storage]
type = "s3"
bucket = "my-live777-bucket"
root = "/recordings"
region = "us-east-1"
access_key_id = "AKIA..."
secret_access_key = "..."
```

Using temporary credentials:
```toml
[recorder.storage]
type = "s3"
bucket = "my-live777-bucket"
root = "/recordings"
region = "us-east-1"
access_key_id = "ASIA..."
secret_access_key = "..."
session_token = "..."
```

### MinIO (S3-Compatible)

```toml
[recorder.storage]
type = "s3"
bucket = "live777-recordings"
root = "/recordings"
region = "us-east-1"
endpoint = "http://localhost:9000"
access_key_id = "minioadmin"
secret_access_key = "minioadmin"
enable_virtual_host_style = false
```


## Start/Status API {#api}

Requires `recorder` feature.

- Start recording: `POST` `/api/record/:streamId`
  - Body (optional): `{ "base_dir": "optional/path/prefix" }` — the body may be omitted entirely (no `Content-Type` needed)
  - Response: `{ "id": ":streamId", "record_id": "<unix-timestamp>", "record_dir": "<path>", "mpd_path": "<path>/manifest.mpd" }`
- Recording status: `GET` `/api/record/:streamId`
  - Response: `{ "recording": true }`
- Stop recording: `DELETE` `/api/record/:streamId`

### Recording Index Sync APIs

- Pull sessions: `GET` `/api/recordings`
  - Query: `?stream=optional&since_ts=0&limit=200`
- ACK sessions: `PATCH` `/api/recordings`
  - Body: `{ "records": [{ "stream": "s", "record": "id" }] }`
- Delete ACKed sessions: `DELETE` `/api/recordings`
  - Body: `{ "records": [{ "stream": "s", "record": "id" }] }` — add `"delete_files": true` to also remove the recording files from storage (the whole record dir of every deleted row); the response then reports `"removed_dirs"` alongside `"deleted"`. Only ACKed rows are ever touched, so files of an active recording are always safe.

## MPD Path Conventions {#mpd}

- Default `record_dir` (when `base_dir` is not provided): `/:streamId/:record_id/` where `record_id` is a 10-digit Unix timestamp (seconds).
- Default MPD location: `/{record_dir}/manifest.mpd`.
- When the cumulative duration for a session reaches `max_recording_seconds`, the recorder closes the current fragments and starts a new timestamped directory (for example `/:streamId/1718200000/`). No calendar-style paths are produced automatically.
- When `base_dir` is provided, `record_dir` matches that value exactly and the manifest lives at `/{base_dir}/manifest.mpd`. If the override does not end with a 10-digit Unix timestamp, the returned `record_id` falls back to the recording start timestamp (seconds).

## File Structure {#file-structure}

Recorded files are organized by `record_dir`:

```
storage/
└── stream1/
    └── 1762842203/
        ├── manifest.mpd
        ├── v_init.m4s
        ├── a_init.m4s
        ├── v_seg_0001.m4s
        ├── a_seg_0001.m4s
        └── ...
```

- Timestamp-based folders (`stream/1762842203`) are the canonical layout produced by Live777, including automatic rotations triggered by `max_recording_seconds`. Provide a custom `base_dir` only if you intentionally need a different structure — note that `record_id` then no longer mirrors the directory name (it falls back to the recording start timestamp).

## Async Upload (Presigned URLs) {#async-upload}

::: warning
Async upload requires S3 storage. It is not supported with the filesystem backend.
:::

Enable async uploads via Liveman presign API and local spool:

```toml
[recorder.upload]
enabled = true
liveman_url = "http://127.0.0.1:8888"
liveman_token = "live777"
queue_path = "./recordings/upload_queue.jsonl"
local_dir = "./recordings"
presign_ttl_seconds = 300
interval_ms = 2000
concurrency = 2
```
