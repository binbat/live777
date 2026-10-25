# WhepWright

`whepwright` is a browser-based WHEP playback tester. It launches a real browser
(Chromium, Firefox, or WebKit) via Playwright, subscribes to a WHEP endpoint,
and verifies that the browser WebRTC stack can receive and render the stream.

For fast, headless decoding with FFmpeg, see [`WhepProbe`](whepprobe).

## Build

```bash
# Requires Node.js and Playwright
cargo build --bin whepwright --features whepwright
```

## Usage

Play a WHEP endpoint in Chromium:

```bash
whepwright -w http://localhost:7777/whep/live
```

Use Firefox and run for up to 60 seconds:

```bash
whepwright -w http://localhost:7777/whep/live --browser firefox --timeout 60
```

Run with a visible browser window for debugging:

```bash
whepwright -w http://localhost:7777/whep/live --headless=false
```

Use the installed Google Chrome for H.265 playback. Headless Chromium does not
support H.265 WebRTC, so you also need a visible window:

```bash
whepwright -w http://localhost:7777/whep/live \
          --browser chromium --channel chrome --headless=false
```

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `-w`, `--whep` | required | WHEP endpoint URL |
| `-t`, `--token` | none | Bearer token for WHEP authentication |
| `--browser` | `chromium` | Browser to use: `chromium`, `firefox`, `webkit` |
| `--channel` | none | Browser channel, e.g. `chrome` or `msedge` (Chromium only) |
| `--headless` | `true` | Run the browser in headless mode (`true` or `false`) |
| `--output` | `human` | Output format: `human`, `json` |
| `--timeout` | `30` | Overall timeout in seconds |
| `--diagnostics-dir` | `whepwright-diagnostics` | Parent directory for per-run diagnostics |
| `--trace` | off | Capture a Playwright trace on failure; cannot be combined with `--token` |

## JSON output

`--output json` writes exactly one JSON result to stdout. Logs, including verbose
Playwright runner logs, go to stderr so stdout can be piped directly to `jq` or
another program. The exit code remains non-zero on playback failure.

The result's `artifacts` object reports the diagnostics `directory`, whether it
was `retained`, the paths of captured files, and any `capture_errors`. A capture
error does not replace the original playback error.

```bash
whepwright -w http://localhost:7777/whep/live --output json
```

## Failure diagnostics

Every invocation uses its own `run-*` directory below `--diagnostics-dir`.
Successful runs remove that directory. Failed runs retain:

- `screenshot.png`: the browser page at the point of failure;
- `console.log`: browser console messages, uncaught page errors, and runner
  messages;
- `sdp-ice.json`: connection state timeline, ICE candidate type counts, the
  selected candidate pair and round-trip time when available, and RTP summaries;
- `trace.zip`: a Playwright trace, only when `--trace` was requested.

`sdp-ice.json` contains summaries, not the offer or answer SDP. Neither
authentication tokens nor complete SDP are written to that file. Capture failures
are listed in the result's `artifacts.capture_errors` array. If the Rust runner
times out, it asks Node to finish capture and close the browser, then cleans up
any remaining runner processes; the retained diagnostics may be incomplete.

Playwright traces can contain evaluated arguments and network authorization
headers. For that reason `--trace --token ...` is rejected before a browser or
diagnostics directory is created. When a bearer token is required, run without
`--trace`.

To retain a trace for a failing run:

```bash
whepwright -w http://localhost:7777/whep/live \
  --output json --trace --diagnostics-dir ./diagnostics
```

## Exit code

- `0`: playback succeeded (WHEP connected and video was rendered).
- `1`: playback failed or an error occurred.
