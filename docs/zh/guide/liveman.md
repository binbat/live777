# Liveman (Live777 Manager)

Live777 集群管理器.

如果我有很多服务器（live777 核心集群），我需要统一管理它们。Liveman 还为整个集群提供集中式录制元数据管理和回放服务。

## 数据库配置

Liveman 现在支持 PostgreSQL 来存储录制元数据。这使得分片信息能够在整个集群中持久化存储。

```toml
[database]
# PostgreSQL 连接 URL
# 默认值: postgresql://localhost/live777
# 环境变量: DATABASE_URL
url = "postgresql://user:password@localhost:5432/live777"

# 最大数据库连接数
# 默认值: 10
max_connections = 10

# 连接超时时间（秒）
# 默认值: 30
connect_timeout = 30
```

## 录制系统

录制系统将分片元数据存储在数据库中，而实际的媒体文件保存在存储系统中（文件系统、S3 等）。

### 分片元数据结构

每个录制的分片包含：
- **ID**：唯一标识符（UUID）
- **节点别名**：录制该分片的 Live777 节点
- **流标识**：流的标识符
- **时间戳**：开始/结束时间戳（微秒）
- **时长**：分片时长（毫秒）
- **路径**：媒体文件的存储路径
- **关键帧**：分片是否以关键帧开始
- **创建时间**：元数据存储的时间

## 认证

### 关闭认证 {#noauth}

参照: [live777#认证/关闭认证](/zh/guide/live777#noauth)

### Bearer token {#token}

参照: [live777#认证/Bearer token](/zh/guide/live777#token)

### JWT(JSON Web Token) {#JWT}

参照: [live777#认证/JWT(JSON Web Token)](/zh/guide/live777#JWT)

### Username/password

登陆 Dashboard，权限和 Bearer token 一样。主要用于管理集群

```toml
[auth]
Admin Dashboard Accounts

[[auth.accounts]]
username = "live777"
password = "live777"

[[auth.accounts]]
username = "live777-2"
password = "live777-2"
```

## 集群模式

集群模式需要 Liveman，我们也可以通过 [`net4mqtt`](/zh/guide/net4mqtt) 来扩展网络

![liveman-cluster](/liveman-cluster.excalidraw.svg)

## 边缘集群 {#verge}

我们支持边缘端和云端混合集群

![live777-verge](/live777-verge.excalidraw.svg)

