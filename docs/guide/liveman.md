# Liveman (Live777 Manager)

Live777 Cluster manager.

If I have so many servers (live777 core cluster), I need this manage them all. Liveman also provides centralized recording index management and playback proxy services for the entire cluster.

## Database Configuration

Liveman stores the recording index (mapping stream + date → mpd_path) in a database. Migrations run automatically at startup.

- Default driver: SQLite (embedded)
- Supported drivers: SQLite, PostgreSQL (via SeaORM `DATABASE_URL`)

```toml
[database]
# Default database URL (SQLite)
# Environment variable: DATABASE_URL
# Default value if not set: sqlite://./liveman.db
url = "sqlite://./liveman.db?mode=rwc"

# Maximum number of database connections
# Default: 10
max_connections = 10

# Connection timeout in seconds
# Default: 30
connect_timeout = 30
```

Example for PostgreSQL

```toml
[database]
url = "postgresql://user:password@localhost:5432/live777"
max_connections = 10
connect_timeout = 30
```

## Recording Index and Storage

The recording system stores the index (date-based manifest location) in the database while keeping the actual media files in storage (filesystem, S3, OSS, etc.).

### Recording Index Schema

Table: `recordings` (auto-created by migrations)

- **id**: UUID, primary key
- **stream**: String, stream identifier
- **year**: Integer
- **month**: Integer
- **day**: Integer
- **mpd_path**: String, path to the manifest within storage
- **created_at**: Timestamp with time zone
- **updated_at**: Timestamp with time zone

Unique index: `(stream, year, month, day)`

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

## Extra `IceServers` {#extra-ice}

This merge all `iceServers` in `WHIP`/`WHEP`

::: danger NOTE:
**This only work `WHIP`/`WHEP` Client**

The `WHIP`/`WHEP` Server don't support trickle ICE. If Server use a lot ice is to slow
:::

If don't need upstream `ice_servers`, You can use `override_upstream_ice_servers = true` for drop

```toml
[extra_ice]
# When WHIP/WHEP use liveman proxy
# liveman override upstream liveion http header link ice_servers
override_upstream_ice_servers = false
```

### Static {#static}

Look like `live777 config [ice_servers]`

```toml
[[extra_ice.ice_servers]]
urls = [
    "stun:stun.22333.fun",
    "stun:cn.22333.fun",
    "stun:stun.l.google.com:19302",
]

[[extra_ice.ice_servers]]
urls = [ "turn:turn.22333.fun", "turn:cn.22333.fun" ]
username = "live777"
credential = "live777"
```

### Coturn {#coturn}

In production environment, We for each session use different turn username / password

::: danger NOTE:
Coturn `--use-auth-secret` and `--lt-cred-mech` is conflict
:::

```toml
[extra_ice.coturn]
# Coturn must use: --use-auth-secret
# The secret is: --static-auth-secret=live777
secret = "live777"
urls = [
    "stun:coturn.22333.fun:3478",
    "turn:coturn.22333.fun:3478?transport=udp",
    "turn:coturn.22333.fun:3478?transport=tcp",
]
ttl = 3600
```

### Cloudflare {#cloudflare}

Use Cloudflare turn servers

```toml
[extra_ice.cloudflare]
# https://developers.cloudflare.com/realtime/turn/generate-credentials/
key_id = ""
api_token = ""
ttl = 3600
```

## Cluster {#cluster}

Cluster mode must liveman. We can use [`net4mqtt`](/guide/net4mqtt) extra network

![liveman-cluster](/liveman-cluster.excalidraw.svg)

## Verge {#verge}

We support cloud and verge mix cluster

![live777-verge](/live777-verge.excalidraw.svg)

