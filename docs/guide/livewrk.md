# LiveWrk

`livewrk` is a load-testing tool for WHIP/WHEP endpoints, named after `wrk`,
the HTTP benchmarking tool. It drives many concurrent publish or subscribe
sessions against a Live777 instance and reports session, traffic, and RTCP
feedback statistics.

## Use cases

- Stress-test a Live777 instance with hundreds of concurrent WHIP publishers.
- Exercise the SFU fan-out path with many WHEP subscribers on one stream.
- Continuously verify that media stays decodable while the system is under
  load (rotating decode verification).

## Build

```bash
# The whip subcommand and WHEP decode verification require FFmpeg
# development libraries (rsmpeg)
cargo build --bin livewrk --features rsmpeg
```

Without the `rsmpeg` feature only the `whep` subcommand (without
`--verify-window`) is available; the `whip` subcommand explains how to
rebuild when invoked.

## Usage

Publish 100 synthetic streams (`load-0` .. `load-99`) for 60 seconds:

```bash
livewrk whip --whip http://localhost:7777/whip/load --sessions 100 --duration 60
```

Subscribe 100 sessions to one already-published stream:

```bash
livewrk whep --whep http://localhost:7777/whep/load-0 --sessions 100 --duration 60
```

The `whip` subcommand appends `-N` to the last path segment of the URL, so
each session publishes its own stream. Point `whep` at one of those streams
(e.g. `load-0`) or at any other published stream.

Ready-made recipes are available in the `justfile`:

```bash
just livewrk-whip 100 60
just livewrk-whep 100 60 load-0
```

## Common options

Both subcommands share these options:

| Option | Default | Description |
|--------|---------|-------------|
| `-w`, `--whip` / `--whep` | required | WHIP/WHEP endpoint URL |
| `-t`, `--token` | none | Bearer token for authentication |
| `--sessions` | `100` | Number of concurrent sessions |
| `--ramp-ms` | `10` | Milliseconds between spawning each session (ramp-up) |
| `--duration` | `60` | Overall run duration in seconds; sessions stop afterwards |
| `-v`, `-vv`, `-vvv` | `warn` | Log verbosity: `info`, `debug`, `trace` |

## `whip` options

The `whip` subcommand publishes synthetic test patterns generated in-process
(same engine as [WhipSynth](./whipsynth)), no external encoder needed.

| Option | Default | Description |
|--------|---------|-------------|
| `--vcodec` | `vp8` | Video codec: `vp8`, `vp9`, `h264`, `h265`, `av1` |
| `--acodec` | none | Audio codec: `opus`, `g722` (omit for no audio) |
| `--width` | `640` | Video width in pixels |
| `--height` | `480` | Video height in pixels |
| `--fps` | `30` | Video frame rate |
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]` (empty string disables ICE servers) |

## `whep` options

| Option | Default | Description |
|--------|---------|-------------|
| `--verify-window` | none | Enable rotating decode verification (seconds per window) |
| `--verify-tolerant` | `false` | Report verification failures without failing the whole run |
| `--ice-server` | `stun:stun.l.google.com:19302` | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]` (empty string disables ICE servers) |

### Rotating decode verification

With `--verify-window N`, a single verifier decodes one session at a time for
N seconds, then rotates to the next active session. Decode cost stays
constant regardless of the session count, so even large runs verify that the
SFU keeps forwarding decodable media. Windows cut short by shutdown do not
count; a run with zero completed windows, or with any failed window, exits
non-zero unless `--verify-tolerant` is set. Codecs without a decoder in the
build are reported in the verification note instead of failing.

## Output

At the end of a run `livewrk` prints a summary:

```
══════════════════════════════════════════════
  whep loadtest results
  Sessions: 100 total, 100 connected, 0 failed, 0 cancelled, 0 aborted
  Packets: 152340, bytes: 52428800 (52.43 MB)
  Avg connected duration: 58.9s
══════════════════════════════════════════════
```

Media write errors and RTCP feedback (NACK/PLI) are shown when non-zero;
`whip` runs report sent packets, `whep` runs received ones.

## Exit code

- `0`: run completed (or interrupted by the first Ctrl-C, after graceful
  shutdown) with at least one connected session and no failed verification.
- `1`: an error occurred, every session failed, or decode verification
  failed without `--verify-tolerant`.
- `130`: a second Ctrl-C forced an immediate quit during shutdown.
