# playwright-whep

A Rust library that drives a real browser with [Playwright](https://playwright.dev)
to perform WHIP publish and/or WHEP subscribe negotiations through a bundled
minimal test page, and reports whether video frames were successfully sent
and/or rendered.

This is useful for automated tests that need to verify browser WebRTC behaviour
end-to-end without building the full Live777 debugger frontend.

## Requirements

- [Node.js](https://nodejs.org/)
- [Playwright](https://playwright.dev) installed in the working directory:
  ```bash
  pnpm add -D playwright
  npx playwright install chromium --no-shell
  ```
  `--no-shell` avoids downloading the separate headless-shell binary; the
  harness launches the full Chromium binary in headless mode instead.
- The WHIP/WHEP endpoint must allow cross-origin requests (CORS) or be served
  from the same origin as the test page.

## Usage

### Subscribe-only

```rust
use std::time::Duration;
use playwright_whep::{WhepBrowserPlayer, Browser};

let result = WhepBrowserPlayer::new("http://localhost:7777/whep/live")
    .browser(Browser::Chromium)
    .timeout(Duration::from_secs(30))
    .headless(true)
    .play()
    .await?;

assert!(result.success);
assert!(result.connected);
assert!(result.video_width > 0);
```

### Publish from the browser

```rust
use std::time::Duration;
use playwright_whep::{WhepBrowserPlayer, Browser, PublishSource};

let result = WhepBrowserPlayer::publish("http://localhost:7777/whip/live")
    .browser(Browser::Chromium)
    .source(PublishSource::FakeCamera)
    .codec("vp8")
    .timeout(Duration::from_secs(30))
    .headless(true)
    .play()
    .await?;

assert!(result.success);
assert!(result.connected);
```

### Publish and subscribe on the same page

```rust
use std::time::Duration;
use playwright_whep::{WhepBrowserPlayer, Browser, PublishSource};

let result = WhepBrowserPlayer::both(
    "http://localhost:7777/whip/live",
    "http://localhost:7777/whep/live",
)
.browser(Browser::Chromium)
.source(PublishSource::FakeCamera)
.codec("vp8")
.timeout(Duration::from_secs(30))
.headless(true)
.play()
.await?;

assert!(result.success);
assert!(result.publish.as_ref().unwrap().success);
assert!(result.subscribe.as_ref().unwrap().success);
```

## How it works

1. The Rust crate resolves the local Playwright module path and writes the
   Node.js runner (`run-playwright.mjs`) and a minimal `player.html` to a
   temporary directory.
2. The Node runner starts a local HTTP server from that temporary directory,
   launches the requested browser, and navigates to `/player.html` with the
   WHIP/WHEP URLs and other options encoded as query parameters.
3. The test page performs the requested publish/subscribe flow and exposes the
   result on `window.__LIVE777_*_RESULT__`.
4. The runner reads the result, wraps it in a mode envelope, and returns it as
   JSON through the Rust API.

## Integration test

The crate is demonstrated in `tests/playwright_whep/`, which:

- starts a `liveion` server with CORS enabled,
- publishes a VP8/H264 test stream via WHIP using an external FFmpeg process,
- runs the browser WHEP player through the bundled test page,
- asserts that the browser successfully receives and renders video.

Run it with:

```bash
cargo test --test playwright_whep --features whepwright
```

The test source requires a working `ffmpeg` binary on `PATH`.
