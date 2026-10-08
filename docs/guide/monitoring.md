# Monitoring

Live777 exposes Prometheus metrics out of the box — the endpoint is always
compiled in, no cargo feature or config flag is needed. A ready-made
Prometheus + Grafana stack ships in the repository for local use and as a
starting point for production deployments.

## Metrics endpoint

Every `live777` node serves its metrics on the HTTP port (default `7777`):

```bash
curl http://localhost:7777/metrics
```

| Metric | Type | Description |
|--------|------|-------------|
| `live777_stream` | gauge | Number of streams |
| `live777_publish` | gauge | Number of publish sessions |
| `live777_subscribe` | gauge | Number of subscribe sessions |
| `live777_reforward` | gauge | Number of reforward (cascade) sessions |
| `live777_rtp_bytes_total{direction="in\|out"}` | counter | RTP media bytes transferred (wire size); `in` = received from publishers, `out` = sent to subscribers |
| `live777_rtcp_packets_total{direction="to_publisher\|from_subscriber", kind="pli\|fir\|nack\|rr\|sr\|twcc\|remb\|other"}` | counter | RTCP packets by direction and type; `to_publisher` = written to a publish peer (including PLI/FIR keyframe requests relayed from subscribers), `from_subscriber` = received from a subscribe peer (requires live777 built with the `source` feature) |
| `live777_stream_rtp_bytes_total{stream, direction="in\|out"}` | counter | Per-stream RTP media bytes — the same accounting as the server-wide counter, with the stream name as a label |
| `live777_stream_sessions{stream, kind="publish\|subscribe"}` | gauge | Per-stream session counts, refreshed from live state on the server's 2 s stats tick |
| `live777_datachannel_messages_total{direction="in\|out"}` | counter | DataChannel messages; `in` = received from a client, `out` = written to a client (one per attached peer, so `out` reflects the broadcast fan-out) |
| `live777_datachannel_bytes_total{direction="in\|out"}` | counter | DataChannel payload bytes (message body, no SCTP/DTLS framing) — the same accounting points as the messages counter |
| `live777_datachannel_dropped_total{direction="out"}` | counter | DataChannel messages dropped because a slow channel lagged behind its stream's broadcast bus |
| `live777_stream_datachannel_bytes_total{stream, direction="in\|out"}` | counter | Per-stream DataChannel payload bytes — the same accounting as the server-wide counter, with the stream name as a label (same cardinality caveats as `stream_rtp_bytes_total`) |

::: warning
The `stream` label is unbounded in deployments where streams are created
dynamically (WHIP/WHEP auto-create is on by default): every live stream adds
new series. A stream's series are removed from the exporter when the stream
is deleted, so the registry stays bounded to live streams — but a
high-churn deployment still shows many short-lived series in Prometheus.
Provisioned-stream deployments (config-declared streams) have a bounded
label set and are the safe target for the per-stream metrics.
:::

Per-stream and per-session bitrates and cumulative totals are also available
as the `stats` field of the stream/session REST API (and pushed live to the
WebUI dashboard over SSE), which is usually more convenient than metrics for
interactive inspection.

::: warning
`live777_rtp_bytes_total{direction="out"}` counts subscriber sessions only.
Traffic sent by static RTP/RTSP output targets (`rtp://` / `rtsp://`
targets) bypasses these counters until
[issue #474](https://github.com/binbat/live777/issues/474) is fixed, so a
stream multicasting at several Mbit/s may show zero outbound traffic.
:::

::: info
`liveman` has no Prometheus endpoint. To monitor a cluster, scrape every
`live777` node directly.
:::

## Prometheus + Grafana stack

The repository ships a self-contained stack:

- `compose.monitoring.yml` — Prometheus + Grafana services
- `conf/monitoring/prometheus.yml` — scrape configuration
- `conf/monitoring/grafana/` — provisioned datasource and a `live777
  overview` dashboard

Start it next to a running live777:

```bash
# live777 itself runs however you like — cargo run, systemd, or Docker
cargo run --release --features=webui

docker compose -f compose.monitoring.yml up -d
```

Then open:

- Grafana: `http://localhost:3000` (default login `admin` / `admin` — change
  it for anything beyond a local deployment)
- Prometheus: `http://localhost:9090`

The default scrape target is `host.docker.internal:7777`, i.e. a live777
running on the Docker host (on Linux the compose file maps
`host.docker.internal` to the host gateway). To monitor several nodes — for
example a local livenil cluster — add them to
`conf/monitoring/prometheus.yml`:

```yaml
scrape_configs:
    - job_name: live777-cluster
      static_configs:
          - targets:
                - host.docker.internal:7777
                - host.docker.internal:7778
                - host.docker.internal:7779
```

The Grafana dashboard shows stream/publisher/subscriber/reforward counts,
sessions over time, RTP bitrate, per-stream bitrate and session counts, and
RTCP packet rates, refreshing every 5 seconds. When Prometheus scrapes
several live777 nodes or a node serves several streams, the `Instance` and
`Stream` dropdowns at the top filter the panels.

## Combining with load tests

Metrics become meaningful under load. Drive the node with
[LiveWrk](./livewrk) while watching the dashboard:

```bash
just livewrk-whip 100 60          # 100 concurrent WHIP publishers
just livewrk-whep 100 60 load-0   # 100 concurrent WHEP subscribers
```

For reproducible benchmarks, fix the parameters (sessions, duration, codec,
resolution), record the `live777_rtp_bytes_total` rate plus process CPU and
memory, and compare across builds.
