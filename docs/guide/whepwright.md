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

## Exit code

- `0`: playback succeeded (WHEP connected and video was rendered).
- `1`: playback failed or an error occurred.
