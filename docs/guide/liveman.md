# Liveman (Live777 Manager)

Live777 Cluster manager.

If I have so many servers (live777 core cluster), I need this manage them all. Liveman also provides centralized recording metadata management and playback services for the entire cluster.

## Database Configuration

Liveman now supports PostgreSQL for storing recording metadata. This enables persistent storage of segment information across the cluster.

```toml
[database]
# PostgreSQL connection URL
# Default: postgresql://localhost/live777
# Environment variable: DATABASE_URL
url = "postgresql://user:password@localhost:5432/live777"

# Maximum number of database connections
# Default: 10
max_connections = 10

# Connection timeout in seconds
# Default: 30
connect_timeout = 30
```

## Recording System

The recording system stores segment metadata in the database while keeping the actual media files in storage (filesystem, S3, etc.).

### Segment Metadata Schema

Each recorded segment contains:
- **ID**: Unique identifier (UUID)
- **Node Alias**: Which Live777 node recorded it
- **Stream**: Stream identifier
- **Timestamps**: Start/end timestamps (microseconds)
- **Duration**: Segment duration in milliseconds
- **Path**: Storage path to the media file
- **Keyframe**: Whether segment starts with keyframe
- **Created At**: When metadata was stored

## Authentication

### No Authentication {#noauth}

References: [live777#Authentication/No Authentication](/guide/live777#noauth)

### Bearer token {#token}

References: [live777#Authentication/Bearer token](/guide/live777#token)

### JWT(JSON Web Token) {#JWT}

References: [live777#Authentication/JWT(JSON Web Token)](/guide/live777#JWT)

### Username/password

Login to dashboard, manager cluster and stream using username/password

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

## Cluster {#cluster}

Cluster mode must liveman. We can use [`net4mqtt`](/guide/net4mqtt) extra network

![liveman-cluster](/liveman-cluster.excalidraw.svg)

## Verge {#verge}

We support cloud and verge mix cluster

![live777-verge](/live777-verge.excalidraw.svg)

