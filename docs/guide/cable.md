# Cable (DataChannel forwarding)

A **cable** is a named, bidirectional datachannel message pipe on a live777
stream, keyed by the channel's `label`. Cables are declared per stream with
`[[stream.<name>.cables]]`.

## Architecture

A cable is **SFU-bridged, not end-to-end**: live777 terminates each peer's
SCTP association and re-broadcasts the messages. Internally each cable is two
broadcast buses — `to_publisher` (messages towards the publisher) and
`to_subscribers` (towards all subscribers) — and every peer channel with a
matching label is bridged onto them by the forward. The publisher's and every
subscriber's channels with that label therefore exchange messages through the
stream.

Consequences to keep in mind:

- There are **no end-to-end ordering/reliability guarantees**: each leg is
  its own SCTP association with its own parameters, and the bus in the middle
  drops a slow receiver's share past its capacity (logged, and counted in the
  DataChannel drop metrics).
- Parameters are **per leg**: a publisher sending unordered on its leg and a
  subscriber receiving ordered on its leg is a legitimate combination.
- Cable traffic does not depend on media: a cable-only stream (no tracks at
  all) is a valid deployment.

## Configuration

```toml
[stream.fpv]
[[stream.fpv.cables]]
label = "control"          # cable identity (channel label), required
# ordered = false          # unset: WebRTC default (ordered, reliable)
# max_retransmits = 0      # partial reliability; mutually exclusive with
                           # max_packet_life_time
# max_packet_life_time = 100
# protocol = "fc-v1"       # sub-protocol, empty by default
# capacity = 1024          # per-cable bus buffer, in messages
# create = true            # default; see "Server-created channels"

# A DC source takes the broadcast seat: its inbound UDP packets are
# broadcast to every subscriber, and subscribers' replies go to its target.
[[stream.fpv.sources]]
[[stream.fpv.sources.cables]]
label = "control"          # which cable this endpoint joins
url = "udp://0.0.0.0:8891?host=127.0.0.1&port=8890"

# A DC target takes the subscriber seat: the publisher's messages go to its
# target, and its inbound packets go to the publisher.
[[stream.fpv.targets]]
[[stream.fpv.targets.cables]]
label = "control"
url = "udp://?host=127.0.0.1&port=8892"  # export-only
```

| field | default | meaning |
| ----- | ------- | ------- |
| `label` | (required) | Cable identity: peer channels created with this label join this cable. |
| `ordered` | unset (WebRTC default: ordered) | `ordered` of the server-created channel. |
| `max_retransmits` | unset (reliable) | Partial reliability by retransmission count; mutually exclusive with `max_packet_life_time`. |
| `max_packet_life_time` | unset | Partial reliability by retransmission lifetime (ms). |
| `protocol` | `""` | Sub-protocol of the server-created channel. |
| `capacity` | `1024` | Broadcast buffer of the cable's two buses, in messages. |
| `create` | `true` | The server creates this cable's channel on every session; `false`: route by label only. |

## Label routing and the default pair

A peer channel whose `label` matches a declared cable joins that cable's
buses. A channel with any other label mixes onto the **anonymous default bus
pair** — the pre-cable behavior — so existing clients keep working on streams
that declare cables.

## Server-created channels (`create = true`)

With the default `create = true`, live777 creates the cable's channel on
every session whose offer carries an `m=application` section, with the
configured parameters. (The DCEP creator declares a channel's parameters;
the peer accepts them read-only. Browsers additionally mirror them onto their
outgoing direction, making the parameters bidirectional there.)

- **Trigger**: the session's first inbound channel OPEN — by then the SCTP
  association is established and the DTLS role resolved, so the server's
  channel ids cannot collide with the client's. (Creating at `Connected`
  would be too early: the SCTP handshake is still in flight.)
- **Dedup**: a client-created channel whose label matches a server-created
  cable is closed; the client must use the server-created channel it receives
  via `ondatachannel`.
- `create = false` turns a cable into pure label routing: endpoints create
  their own channels with their own parameters.

## Cable <-> UDP endpoints

DC endpoints are declared as `cables` entries on the stream's `sources` and
`targets`, each carrying the cable `label` it joins plus a `udp://` URL:

- A **DC source** takes the broadcast seat: its inbound UDP packets (the
  `udp://<listen>` authority) are delivered to all subscribers, and
  subscribers' messages are sent to its `?host=`/`?port=` target.
- A **DC target** takes the subscriber seat: the publisher's messages are
  sent to its target, and its inbound packets are delivered to the publisher.

`udp://` authority (local bind) and the `host`/`port` query pair (fixed
destination) are independently optional, so one-way (inject-only /
export-only) endpoints are expressible; an omitted target means
reply-to-sender. Endpoints own their listen ports for the stream's whole
lifetime — a provisioned-stream reset does not rebind them.

## HTTP inject and receive

Application servers can inject a message into a stream's cable over HTTP,
without any UDP bridge:

```
POST /api/streams/{stream}/cables/{label}/messages
{ "data": "<utf-8 string>" }
```

- Without a query string the message is **broadcast** onto the resolved
  bus's `to_subscribers` side — every subscriber's channel with this label
  receives it, exactly as if the publisher had sent it. `label` needs no
  declared cable: unlisted labels resolve to the anonymous default bus pair
  (where *all* undeclared channels mix — see Limitations). The response is
  `200 { "receivers": <int> }` with the bus's live receiver count (0 when
  nothing is listening).
- With `?session=<sessionId>` the message bypasses the bus and is sent
  **directly** onto that session's open channel with this label; the
  response reports `receivers: 1`. Errors: `404` when the session has no
  such channel (not bridged yet, or closed for good), `409` when the
  channel exists but is still connecting (retry — it opens within a
  round-trip), `400` when the payload exceeds that channel's negotiated
  max message size (browsers negotiate 256 KiB; the bus itself is
  byte-unbounded).

The complementary receive direction streams what peers send towards the
publisher seat, as SSE:

```
GET /api/streams/{stream}/cables/{label}/messages
```

The GET subscribes the resolved bus's `to_publisher` side — the exact
counterpart of POST's `to_subscribers` inject: messages from peer channels
(and DC-target UDP injects) flowing towards the publisher. One SSE event
per bus message, JSON-encoded: `{ "data": "<base64>" }` for a payload (bus
messages are opaque bytes), `{ "lagged": n }` after the receiver fell
behind and dropped `n` messages (the stream continues). The event stream
ends when the stream is deleted or the server shuts down. Senders are not
tagged on the bus, so there is no `?session=` filter on receive.

Delivery to peer channels is a **binary** datachannel frame — the bus
carries `Vec<u8>`, so the text/binary distinction is lost even though the
request body is a JSON string. Browser clients must handle
`Blob`/`ArrayBuffer` in `onmessage` (e.g. `await event.data.text()`), not
assume a DOM string.

Both methods need a JWT with the `x` scope (static-token and no-auth
deployments pass as-is): the GET exposes *every* peer's messages towards
the publisher — traffic a plain WHEP subscriber (scope `r`) could never
see — so receive is an operator/API-server capability, not a viewer one.
See [Live777 HTTP API](./live777-api.md#cable).

## Limitations

- `negotiated: true` (out-of-band) channels are not supported.
- The offer must carry `m=application` (an answer cannot add m-lines) —
  clients get it by creating at least one channel before offering.
- On Rust (rtc-stack) peers, an *accepted* channel's reverse direction stays
  reliable/ordered until the upstream accept-mirror fix lands; browser peers
  mirror correctly, so server-created cables are fully effective there.
- No end-to-end guarantees across the SFU: reliability parameters apply per
  leg, and bus overflow drops are logged per receiver.
- A message larger than a receiver's negotiated max message size is dropped
  **for that receiver only** (logged and counted in
  `datachannel_dropped_total`); the channel and all other receivers are
  unaffected. Browsers negotiate 256 KiB — keep bus payloads below that
  when every subscriber must receive them.
- A peer opening several channels with the *same* label registers only the
  newest one for targeted `?session=` injects; bus fan-out still bridges
  every channel.
- The anonymous default bus pair mixes *all* channels whose label matches
  no declared cable: an inject (or a GET) on an undeclared label reaches
  every such channel, whatever its actual label.
- Bus messages — HTTP-injected ones included — are delivered as **binary**
  datachannel frames: the bus carries `Vec<u8>`, so the text/binary
  distinction is lost inside the SFU. Browser clients must handle
  `Blob`/`ArrayBuffer` in `onmessage` (e.g. `await event.data.text()`),
  not assume a DOM string.

## Clients

`whipinto --cable-url udp://...` and `whepfrom --cable-url udp://...` bridge
their control channel to UDP as before (`--cable-label` selects a specific
cable instead of the default); both also accept server-created cable
channels automatically (the bridge follows whichever channel is currently
open). Browser clients just handle `ondatachannel` for server-created cables:

```js
pc.ondatachannel = (ev) => {
  // ev.channel.label === "control", parameters as configured on the server
  ev.channel.onmessage = (m) => { /* ... */ };
};
```
