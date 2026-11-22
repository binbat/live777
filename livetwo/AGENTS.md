# livetwo — Agent Guide

Notes for the WHIP/WHEP ↔ RTP/RTSP protocol-conversion library used by
`whipinto`/`whepfrom` and liveion's `cascade`/source/target features. The
root [AGENTS.md](../AGENTS.md) covers repo-wide conventions — read it first.
When you change behavior documented here, update this file.

## Module Map

- `src/whip/core.rs` — the single WHIP publish core (peer construction,
  connection waits, ICE diagnostics) shared by the RTP/RTSP bridge
  (`whipinto`) and the synthetic `whipsynth` publisher.
- `src/probe/worker.rs` — isolated FFmpeg decode worker: FFmpeg decoding for
  `whepprobe` and livewrk's rotating verifier runs in separate `whepprobe
  --decode-worker` processes, so a stuck decoder can be killed and reaped
  without leaking threads. Termination and reaping are bounded
  (`REAP_TIMEOUT`, 5 s): a child that somehow survives `SIGKILL` is
  abandoned and left to tokio's orphan queue instead of pinning the probe or
  the single reaper thread. `livewrk` finds the helper beside its executable
  (or via `LIVE777_WHEPPROBE_BIN`); in test runs the helper is found at the
  cargo profile root, one level above `deps/` (legacy layout) or above
  `build/<pkg>/<hash>/out/` (build-dir layout v2, cargo ≥ 1.100);
  diagnostics bundles ship both binaries together.
- `src/datachannel/` (`livetwo::datachannel`) — client-side DataChannel
  bridges for whepfrom and whipinto (UDP bridge, channel poll loop,
  `CancelOnDrop`), mirroring the server-side cable hub (see
  [liveion/AGENTS.md](../liveion/AGENTS.md)). The bridges run on a
  session-scoped token that `whep::from` / `whip::into` cancels and awaits
  before returning; both accept server-created cable channels (a
  `CableRouter` retargets the outbound queue to the currently open channel).
  On the WHIP leg the "control" DataChannel is opt-in (`whipinto
  --cable-url`) because it adds an `m=application` section to the offer; the
  WHEP leg creates it unless `WhepPeerOptions::control_channel` is disabled.
- `src/transport/multicast.rs` (`livetwo::transport::multicast`, `multicast`
  feature) — multicast sender/receiver socket builders, interface resolution
  (v4 address vs v6 index/name via `if_nametoindex`) and the dual-stack RTCP
  socket, shared by liveion's `target-rtp`, `source-sdp` and whepfrom's
  `rtp://group:port` output. The root crate enables the feature for livetwo,
  so whepfrom always has it: video rides the URL port, audio port + 2,
  `?ttl=`/`?interface=` tune the group send; the legacy
  `?video=`/`?audio=` query-port form and `sdp://` are unchanged.
- `RePayloadCodec` — RTP re-packetization (SPS/PPS inlined ahead of every
  IDR, seeded from the codec fmtp via `with_sprop_params`) used by liveion's
  media output targets; see the target sections in
  [liveion/AGENTS.md](../liveion/AGENTS.md).
