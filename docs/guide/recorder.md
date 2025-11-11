# Recorder

The Recorder in liveion is an optional feature that automatically records live streams into MP4 fragments and saves them locally or to the cloud. The `recorder` feature must be enabled at compile time.

## Supported Codecs {#codec}

| container           | video codecs | audio codecs |
| ------------------- | ------------| ------------ |
| `Fragmented MP4`    | `H264`, `VP9`| `Opus`       |

**Recorder does not support the `VP8` codec because `VP8` requires a `WebM` container.**

## Liveman Integration {#liveman}

Integrates with [Liveman](/guide/liveman) for centralized playback and proxy access:

- Start recording returns the storage metadata (`record_id`, `record_dir`, and `mpd_path`). The `record_id` field is only populated when the recorder can infer a 10-digit Unix timestamp from the output path; otherwise it is returned as an empty string.
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

# Storage backend configuration
[recorder.storage]
type = "fs"          # Storage type: "fs", "s3", or "oss"
root = "./records"   # Root path for recordings
```

## Storage Backends {#storage}

### Local File System

```toml
[recorder.storage]
type = "fs"
root = "/var/lib/live777/recordings"
```

## Start/Status API {#api}

Requires `recorder` feature.

- Start recording: `POST` `/api/record/:streamId`
  - Body (optional): `{ "base_dir": "optional/path/prefix" }`
  - Response: `{ "id": ":streamId", "record_id": "<unix-timestamp-or-empty>", "record_dir": "<path>", "mpd_path": "<path>/manifest.mpd" }`
- Recording status: `GET` `/api/record/:streamId`
  - Response: `{ "recording": true }`
- Stop recording: `DELETE` `/api/record/:streamId`

## MPD Path Conventions {#mpd}

- Default `record_dir` (when `base_dir` is not provided): `/:streamId/:record_id/` where `record_id` is a 10-digit Unix timestamp (seconds).
- Default MPD location: `/{record_dir}/manifest.mpd`.
- When the cumulative duration for a session reaches `max_recording_seconds`, the recorder closes the current fragments and starts a new timestamped directory (for example `/:streamId/1718200000/`). No calendar-style paths are produced automatically.
- When `base_dir` is provided, `record_dir` matches that value exactly and the manifest lives at `/{record_dir}/manifest.mpd`. If the override does not end with a 10-digit Unix timestamp, the returned `record_id` is an empty string.

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

### Alibaba Cloud OSS

```toml
[recorder.storage]
type = "oss"
bucket = "my-oss-bucket"
root = "/recordings"
region = "oss-cn-hangzhou"
endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
access_key_id = "your-access-key"
access_key_secret = "your-access-secret"
```

For STS temporary credentials:
```toml
[recorder.storage]
type = "oss"
bucket = "my-oss-bucket"
root = "/recordings"
region = "oss-cn-hangzhou"
endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
access_key_id = "STS..."
access_key_secret = "..."
security_token = "..."
```

## Configuration Options {#options}

### S3 Backend Options

- `bucket`: S3 bucket name (required)
- `root`: Root path within bucket (default: "/")
- `region`: AWS region (optional, auto-detected if not set)
- `endpoint`: Custom endpoint for S3-compatible services
- `access_key_id`: AWS access key ID
- `secret_access_key`: AWS secret access key
- `session_token`: Session token for temporary credentials
- `disable_config_load`: Disable automatic credential loading from environment/files
- `enable_virtual_host_style`: Enable virtual-hosted-style requests (required for some S3-compatible services)

### OSS Backend Options

- `bucket`: OSS bucket name (required)
- `root`: Root path within bucket (default: "/")
- `region`: OSS region (required)
- `endpoint`: OSS endpoint (required)
- `access_key_id`: Alibaba Cloud access key ID
- `access_key_secret`: Alibaba Cloud access key secret
- `security_token`: Security token for STS temporary credentials

## File Structure {#file-structure}

Recorded files are organized by `record_dir`:

```
records/
└── stream1/
    └── 1762842203/
        ├── manifest.mpd
        ├── v_init.m4s
        ├── a_init.m4s
        ├── v_seg_0001.m4s
        ├── a_seg_0001.m4s
        └── ...
```

- Timestamp-based folders (`stream/1762842203`) are the canonical layout produced by Live777, including automatic rotations triggered by `max_recording_seconds`. Provide a custom `base_dir` only if you intentionally need a different structure and accept the impact on `record_id` values.
