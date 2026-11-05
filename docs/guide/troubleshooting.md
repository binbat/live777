---
description: >-
  Diagnose WHIP/WHEP bridge failures: authentication, HTTP status codes, ICE
  connectivity and missing media.
---

# Troubleshooting

Checks shared by `whipinto`, `whepfrom` and the probe tools. Start with `-vv`:
the WebRTC and bridge diagnostics normally show which stage failed.

## Authentication and HTTP status

| Symptom | What to check |
|---------|---------------|
| `401 Unauthorized` | Pass `-t <token>` and make sure it exactly matches the server's `[auth] tokens`; do not add a second `Bearer` prefix. |
| `403 Forbidden` | Authentication reached the server but a proxy or cluster policy rejected it; check Live777/liveman authorization and routing rules. |
| `409 Conflict` | A publisher, session or configured source already owns the stream. Stop it, use another stream, or review `strategy.override_publisher`; cascade-pull publishers cannot be displaced. |
| `503 Service Unavailable` | The cluster has no available node with remaining subscriber capacity; check node health, capacity limits and scheduling policy. |

A `404` means the endpoint path itself is wrong (`/whip/<stream>` versus
`/whep/<stream>`), the stream is missing under a policy that disables automatic
creation, or a reverse proxy is rewriting the path. Each tool page lists what
it expects.

## ICE timeout or disconnected

Host candidates alone only work where the client and the server can reach each
other directly. For remote or NAT deployments, configure server ICE addresses
and STUN/TURN, then pass `--ice-server` when the client needs it. See
[live777 configuration](/guide/live777).

## Connected, but no media

Confirm the upstream direction is actually producing media: a publisher is
attached for a subscriber, the sender emits to the SDP ports, and the SDP
codecs match the encoder output. If the tool reports frames decoded but the
receiver shows nothing, the problem is downstream of Live777 — check the
receiver's SDP and firewall rules.
