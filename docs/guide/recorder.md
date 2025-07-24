# Recorder

The Recorder in liveion is an optional feature that automatically records live streams into MP4 fragments and saves them locally or to the cloud. The `recorder` feature must be enabled at compile time.

## Supported Codecs {#codec}

| container           | video codecs | audio codecs |
| ------------------- | ------------| ------------ |
| `Fragmented MP4`    | `H264`      | `Opus`       |
| `WebM`              |             |              |

## Configuration {#config}

Configure recording parameters in `live777.toml`:

```toml
[recorder]
# Stream name patterns for auto-recording, supports wildcards
auto_streams = ["*"]  # Record all streams
# auto_streams = ["room1", "room2", "web-*"]  # Record specific streams

# Storage location
root = "file://./records"  # Local storage
# root = "s3://bucket/path?region=us-east-1&access_key_id=xxx&secret_access_key=yyy"  # S3 storage
```

## Storage Options {#storage}

### Local File System
```toml
[recorder]
root = "file:///var/lib/live777/recordings"
```

### AWS S3
```toml
[recorder]
# Use IAM role
root = "s3://my-bucket/recordings?region=us-east-1"

# Use explicit credentials
root = "s3://my-bucket/recordings?region=us-east-1&access_key_id=AKIA...&secret_access_key=..."
```

### S3-Compatible Services
```toml
[recorder]
# MinIO
root = "s3://bucket/path?region=us-east-1&endpoint=http://localhost:9000&access_key_id=admin&secret_access_key=password"

# Alibaba Cloud OSS
root = "s3://bucket/path?region=oss-cn-hangzhou&endpoint=https://oss-cn-hangzhou.aliyuncs.com&access_key_id=xxx&secret_access_key=yyy"
```

## File Structure {#file-structure}

Recorded files are organized as follows:

```
records/
├── stream1/
│   └── 2025/
│       └── 07/
│           └── 24/
│               ├── manifest.mpd
│               ├── init.m4s
│               ├── audio_init.m4s
│               ├── seg_0001.m4s
│               ├── audio_seg_0001.m4s
│               └── ...
└── stream2/
    └── 2025/
        └── 07/
            └── 24/
                └── ...
```
