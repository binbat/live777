# Recorder

liveion 的 Recorder 是一个可选功能，用于将实时流自动录制为 MP4 分片并保存到存储。需要在编译时启用 `recorder` 特性。

## 目前支持的编码 {#codec}

| container        | video codecs         | audio codecs |
| ---------------- | -------------------- | ------------ |
| `Fragmented MP4` | `H264`, `H265`, `VP9`, `AV1` | `Opus`       |

**Recorder 暂不支持 `VP8` 编码，因为 `VP8` 需要 `WebM` 容器。**对视频编码不受支持的流启动录制会立即返回 `400 Bad Request`，而不是静默地录出无视频的会话。

## Liveman 集成 {#liveman}

与 [Liveman](/zh/guide/liveman) 集成以实现集中式回放和代理访问：

- 启动录制时 Live777 会返回存储元数据（`record_id`、`record_dir`、`mpd_path`）。`record_id` 取自输出路径中的 10 位 Unix 时间戳；路径中没有时回退为录制启动时间戳（秒），不会为空。
- Liveman 使用 `record_id`/`record_dir` 与存储保持一致，再通过 `mpd_path` 回放
- 媒体文件可通过 Liveman 代理获取：`GET /api/record/object/{path}`

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
# 自动录制的流名称模式，支持通配符（默认：空列表）
auto_streams = ["*"]                # 录制所有流
# auto_streams = ["room1", "web-*"]   # 仅录制指定流

# 单个录制会话的最大持续时间（秒），超过即重新开一个录制（默认：86_400）
max_recording_seconds = 86_400

# 可选：多节点部署的节点别名
node_alias = "live777-node-001"

# 存储后端配置（默认：本地文件系统）
[recorder.storage]
type = "fs"
root = "./storage"
```

### 配置选项

#### 基础选项

- `auto_streams`: 自动录制的流名称模式，支持通配符（默认：`[]` 空列表）
- `max_recording_seconds`: 单个录制会话的最大持续时间（秒），超过即重新开一个录制（默认：`86400`，设为 `0` 禁用自动轮转）
- `node_alias`: 可选的节点标识符，用于多节点部署（默认：不设置）

#### 存储选项

**本地文件系统后端（默认）：**

- `type`: `"fs"`
- `root`: 存储录制文件的根目录（默认：`"./storage"`）

::: warning
本地文件系统后端仅支持基本录制功能。[异步上传队列](#async-upload)功能需要 S3 存储。
:::

**S3 后端：**

- `type`: `"s3"`
- `bucket`: S3 存储桶名称（必需）
- `root`: 存储桶内的根路径（默认：`"/"`）
- `region`: AWS 区域（可选，未设置时从环境自动检测）
- `endpoint`: S3 兼容服务的自定义端点 URL（可选）
- `access_key_id`: AWS 访问密钥 ID（可选，可从环境加载）
- `secret_access_key`: AWS 访问密钥 Secret（可选，可从环境加载）
- `session_token`: 临时凭证的会话令牌（可选）
- `disable_config_load`: 设为 `true` 禁用从环境/配置文件自动加载凭证（默认：`false`）
- `enable_virtual_host_style`: 启用虚拟主机样式请求，如 `bucket.endpoint.com` 而非 `endpoint.com/bucket`（默认：`false`）

## 存储后端 {#storage}

### 本地文件系统（默认）

```toml
[recorder.storage]
type = "fs"
root = "./storage"
```

::: info
本地文件系统后端支持基本录制（写入分片、更新 MPD）。不支持预签名 URL 和异步上传队列。
:::

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


## 启动/状态 API {#api}

需要启用 `recorder` 特性。

- 启动录制: `POST` `/api/record/:streamId`
  - 请求体（可选）: `{ "base_dir": "optional/path/prefix" }` —— 请求体可完全省略（无需 `Content-Type`）
  - 响应: `{ "id": ":streamId", "record_id": "<10位Unix时间戳>", "record_dir": "<path>", "mpd_path": "<path>/manifest.mpd" }`
- 录制状态: `GET` `/api/record/:streamId`
  - 响应: `{ "recording": true }`
- 停止录制: `DELETE` `/api/record/:streamId`

### 录制索引同步 API

- 拉取会话：`GET` `/api/recordings`
  - Query：`?stream=optional&since_ts=0&limit=200`
- ACK 会话：`PATCH` `/api/recordings`
  - 请求体：`{ "records": [{ "stream": "s", "record": "id" }] }`
- 删除已 ACK 会话：`DELETE` `/api/recordings`
  - 请求体：`{ "records": [{ "stream": "s", "record": "id" }] }` —— 附加 `"delete_files": true` 可连同存储中的录制文件一起删除（每个被删行的整个录制目录）；响应在 `"deleted"` 之外会附带 `"removed_dirs"`。只有已 ACK 的行会被处理，进行中的录制文件始终不受影响。

## MPD 路径规则 {#mpd}

- 默认 `record_dir`（未显式指定 `base_dir` 时）为 `/:streamId/:record_id/`，其中 `record_id` 是 10 位 Unix 时间戳。
- 默认 MPD 位置： `/{record_dir}/manifest.mpd`。
- 当单个录制会话累计时长达到 `max_recording_seconds` 时，Recorder 会关闭当前片段并以新的时间戳目录（如 `/:streamId/1718200000/`）继续录制，系统不会自动生成日历路径。
- 当提供 `base_dir` 时，`record_dir` 与该值完全一致，Manifest 位于 `/{base_dir}/manifest.mpd`。若该值未以 10 位 Unix 时间戳结尾，响应中的 `record_id` 回退为录制启动时间戳（秒）。

## 文件组织结构 {#file-structure}

录制文件会根据 `record_dir` 组织：

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

- 时间戳目录（如 `stream1/1762842203`）是 Live777 的唯一默认布局，也覆盖了 `max_recording_seconds` 触发的自动轮转。仅在非常明确的场景下才覆盖 `base_dir`，并留意此时 `record_id` 不再与目录名一致（回退为录制启动时间戳）。

## 异步上传（预签名） {#async-upload}

::: warning
异步上传需要 S3 存储，本地文件系统后端不支持此功能。
:::

通过 Liveman 预签名接口和本地落盘队列异步上传：

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
