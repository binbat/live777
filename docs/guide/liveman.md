# Liveman (Live777 Manager)

Live777 Cluster manager.

If I have so many servers (live777 core cluster), I need this manage them all.

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

