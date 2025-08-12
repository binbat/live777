# Liveman (Live777 Manager)

Live777 集群管理器.

如果我有很多服务器（live777 核心集群），我需要统一管理它们。Liveman 还为整个集群提供集中式录制索引管理和回放代理服务。

## 数据库配置

Liveman 将录制索引（stream + 日期 → mpd_path 的映射）存储在数据库中。程序启动时会自动执行迁移。

- 默认驱动：SQLite（嵌入式）
- 支持驱动：SQLite、PostgreSQL（通过 SeaORM `DATABASE_URL`）

```toml
[database]
# 默认数据库 URL（SQLite）
# 环境变量: DATABASE_URL
# 未设置时默认值: sqlite://./liveman.db
url = "sqlite://./liveman.db?mode=rwc"

# 最大数据库连接数
# 默认值: 10
max_connections = 10

# 连接超时时间（秒）
# 默认值: 30
connect_timeout = 30
```

PostgreSQL 示例：

```toml
[database]
url = "postgresql://user:password@localhost:5432/live777"
max_connections = 10
connect_timeout = 30
```

## 录制索引与存储

录制系统在数据库中存储日期索引（manifest 位置），而实际媒体文件保存在存储系统（文件系统、S3、OSS 等）。

### 录制索引表结构

表名：`recordings`（由迁移自动创建）

- **id**：UUID，主键
- **stream**：字符串，流标识
- **year**：整型
- **month**：整型
- **day**：整型
- **mpd_path**：字符串，manifest 在存储中的路径
- **created_at**：带时区时间戳
- **updated_at**：带时区时间戳

唯一索引：`(stream, year, month, day)`

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

