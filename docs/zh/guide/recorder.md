# Recorder

liveion 的 Recorder 是一个可选功能，用于将实时流自动录制为 MP4 分片并保存到本地或云。需要在编译时启用 `recorder` 特性。

## 目前支持的编码 {#codec}

| container  | video codecs                | audio codecs   |
| -------- | --------------------------- | -------------- |
| `Fragmented MP4`   | `H264` | `Opus` |
| `WebM`   |   |  |

## 配置说明 {#config}

在 `live777.toml` 中配置录制参数：

```toml
[recorder]
# 自动录制的流名称模式，支持通配符
auto_streams = ["*"]  # 录制所有流
# auto_streams = ["room1", "room2", "web-*"]  # 录制指定流

# 存储位置配置
root = "file://./records"  # 本地存储
# root = "s3://bucket/path?region=us-east-1&access_key_id=xxx&secret_access_key=yyy"  # S3 存储
```

## 存储支持 {#storage}

### 本地文件系统
```toml
[recorder]
root = "file:///var/lib/live777/recordings"
```

### AWS S3
```toml
[recorder]
# 使用 IAM 角色
root = "s3://my-bucket/recordings?region=us-east-1"

# 使用显式凭证
root = "s3://my-bucket/recordings?region=us-east-1&access_key_id=AKIA...&secret_access_key=..."
```

### S3 兼容服务
```toml
[recorder]
# MinIO
root = "s3://bucket/path?region=us-east-1&endpoint=http://localhost:9000&access_key_id=admin&secret_access_key=password"

# 阿里云 OSS
root = "s3://bucket/path?region=oss-cn-hangzhou&endpoint=https://oss-cn-hangzhou.aliyuncs.com&access_key_id=xxx&secret_access_key=yyy"
```

## 文件组织结构 {#file-structure}

录制文件按以下结构自动组织：

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
