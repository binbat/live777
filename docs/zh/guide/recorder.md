# Recorder

liveion 的 Recorder 是一个可选功能，用于将实时流自动录制为 MP4 分片并保存到本地或云。需要在编译时启用 `recorder` 特性。

## 目前支持的编码 {#codec}

| container  | video codecs                | audio codecs   |
| -------- | --------------------------- | -------------- |
| `Fragmented MP4`   | `H264` | `Opus` |
| `WebM`   |   |  |

## Liveman 集成 {#liveman}

录制器可以与 [Liveman](/zh/guide/liveman) 集成，为整个 Live777 集群提供集中化的元数据管理和回放服务。

### 元数据管理

录制器将分片元数据存储在内存中，Liveman 可以通过拉取 API 定期获取这些数据，包括：

- 流标识符和节点别名
- 分片时间戳（开始/结束时间，以微秒为单位）
- 分片时长和文件路径
- 关键帧信息

这使得集群范围内的录制管理和基于时间轴的回放成为可能。

### 配置

```toml
[recorder]
# 可选：节点别名，用于在集群中标识此 Live777 实例
node_alias = "live777-node-001"
```

::: tip 注意
node_alias 是可选的，但在多节点部署中建议配置，以帮助 Liveman 识别录制元数据的来源。
:::

## 配置说明 {#config}

在 `live777.toml` 中配置录制参数：

```toml
[recorder]
# 自动录制的流名称模式，支持通配符
auto_streams = ["*"]  # 录制所有流
# auto_streams = ["room1", "room2", "web-*"]  # 录制指定流

# 可选：多节点部署的节点别名
node_alias = "live777-node-001"

# 存储后端配置
[recorder.storage]
type = "fs"  # 存储类型: "fs", "s3", 或 "oss"
root = "./records"  # 录制文件根路径
```

## 存储后端 {#storage}

### 本地文件系统

```toml
[recorder.storage]
type = "fs"
root = "/var/lib/live777/recordings"
```

### AWS S3

使用 IAM 角色（推荐用于 EC2/ECS）：
```toml
[recorder.storage]
type = "s3"
bucket = "my-live777-bucket"
root = "/recordings"
region = "us-east-1"
```

使用显式凭证：
```toml
[recorder.storage]
type = "s3"
bucket = "my-live777-bucket"
root = "/recordings"
region = "us-east-1"
access_key_id = "AKIA..."
secret_access_key = "..."
```

使用临时凭证：
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

### MinIO（S3兼容）

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

### 阿里云 OSS

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

使用 STS 临时凭证：
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

## 配置选项 {#options}

### S3 后端选项

- `bucket`: S3 存储桶名称（必需）
- `root`: 存储桶内的根路径（默认："/"）
- `region`: AWS 区域（可选，未设置时自动检测）
- `endpoint`: S3兼容服务的自定义端点
- `access_key_id`: AWS 访问密钥 ID
- `secret_access_key`: AWS 访问密钥 Secret
- `session_token`: 临时凭证的会话令牌
- `disable_config_load`: 禁用从环境/文件自动加载凭证
- `enable_virtual_host_style`: 启用虚拟主机样式请求（某些S3兼容服务需要）

### OSS 后端选项

- `bucket`: OSS 存储桶名称（必需）
- `root`: 存储桶内的根路径（默认："/"）
- `region`: OSS 区域（必需）
- `endpoint`: OSS 端点（必需）
- `access_key_id`: 阿里云访问密钥 ID
- `access_key_secret`: 阿里云访问密钥 Secret
- `security_token`: STS 临时凭证的安全令牌

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
