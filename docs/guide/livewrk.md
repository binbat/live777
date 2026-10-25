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
| `--output` | `human` | Report format: `human` or `json` |
| `--min-connected` | `0` | Minimum connected sessions required for success |
| `--min-packets` | `0` | Minimum aggregate packets required for success |
| `--min-bytes` | `0` | Minimum aggregate bytes required for success |
| `--fail-on-any-session` | `false` | Fail if any session fails or is aborted |
| `-v`, `-vv`, `-vvv` | `warn` | Log verbosity: `info`, `debug`, `trace` |

The three thresholds apply to the final aggregate report, not to each session.
Without them, a run with at least one connected session keeps the original
partial-failure behavior and exits successfully; failed and aborted sessions
are listed as warnings. All sessions failing is always an error. Cleanly
cancelled sessions do not count as failures.

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
| `--ice-server` | none | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]`. Defaults to none — host candidates only (WHIP/WHEP endpoints advertise their own ICE servers via Link headers) |

## `whep` options

| Option | Default | Description |
|--------|---------|-------------|
| `--verify-window` | none | Enable rotating decode verification (seconds per window) |
| `--verify-tolerant` | `false` | Report verification failures without failing the whole run |
| `--ice-server` | none | ICE server for gathering, repeatable; format `<url>[,<username>[,<credential>]]`. Defaults to none — host candidates only (WHIP/WHEP endpoints advertise their own ICE servers via Link headers) |

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

With `--output json`, stdout contains only one JSON report and logs go to
stderr. Schema version 1 has the following top-level fields (the same fields
are emitted for successful and failed runs):

```json
{
  "schema_version": 1,
  "stats": {
    "sessions_total": 2,
    "sessions_connected": 2,
    "sessions_failed": 0,
    "sessions_cancelled": 0,
    "sessions_aborted": 0,
    "total_packets": 152340,
    "total_bytes": 52428800,
    "total_errors": 0,
    "total_nack_count": 0,
    "total_pli_count": 0,
    "total_connected_duration": { "secs": 20, "nanos": 0 }
  },
  "verification": null,
  "failures": [],
  "warnings": [],
  "success": true
}
```

`verification` is `null` when verification was not requested or its results
were lost; otherwise it contains the rotating verifier's window, session,
frame, note, and error statistics. `failures` explains unsuccessful acceptance
checks, while `warnings` records tolerated verification problems, partial
session failures, and runs where every session was cleanly cancelled. Even an
all-failed run emits this report before exiting non-zero. Runtime errors that
occur before collection (for example, an invalid WHIP URL or unavailable decode
verification support) also emit this envelope in JSON mode: `success` is
`false`, `failures` contains the error, and `stats` and `verification` use their
default empty values. Command-line parsing errors happen before JSON mode is
established and retain the standard CLI error output.

## Exit code

- `0`: all configured acceptance checks passed (or the first Ctrl-C triggered
  a graceful shutdown whose final report passed). Partial session failures are
  allowed by default; verification failures can be tolerated explicitly.
- `1`: an operational error occurred, every session failed, a configured
  minimum was not met, `--fail-on-any-session` observed a failed/aborted
  session, or decode verification failed without `--verify-tolerant`.
- `130`: a second Ctrl-C forced an immediate quit during shutdown.
