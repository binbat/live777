// Playwright runner for the Live777 minimal WebRTC test player.
// This script is invoked from Rust; see libs/playwright-whep/src/lib.rs.

import http from "node:http";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

function parseArgs() {
  const args = process.argv.slice(2);
  const result = {
    whepUrl: "",
    whipUrl: "",
    streamId: "",
    mode: "subscribe",
    timeoutMs: 30000,
    browser: "chromium",
    channel: "",
    headless: true,
    staticRoot: "",
    source: "fake",
    vcodec: "",
    acodec: "",
    layer: "",
    token: "",
  };
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i];
    const value = args[i + 1];
    switch (key) {
      case "--whep-url":
        result.whepUrl = value;
        break;
      case "--whip-url":
        result.whipUrl = value;
        break;
      case "--stream-id":
        result.streamId = value;
        break;
      case "--mode":
        result.mode = value;
        break;
      case "--timeout":
        result.timeoutMs = parseInt(value, 10);
        break;
      case "--browser":
        result.browser = value;
        break;
      case "--channel":
        result.channel = value;
        break;
      case "--headless":
        result.headless = value === "true";
        break;
      case "--static-root":
        result.staticRoot = value;
        break;
      case "--source":
        result.source = value;
        break;
      case "--vcodec":
        result.vcodec = value;
        break;
      case "--acodec":
        result.acodec = value;
        break;
      case "--layer":
        result.layer = value;
        break;
      case "--token":
        result.token = value;
        break;
    }
  }
  return result;
}

function loadPlaywright(browserName) {
  const modulePath = process.env.PLAYWRIGHT_MODULE_PATH;
  if (!modulePath) {
    throw new Error("PLAYWRIGHT_MODULE_PATH environment variable is not set");
  }
  const require = createRequire(import.meta.url);
  const playwright = require(modulePath);
  const launcher = playwright[browserName];
  if (!launcher) {
    throw new Error(`Unsupported browser: ${browserName}`);
  }
  return launcher;
}

function guessContentType(filePath) {
  const ext = path.extname(filePath).toLowerCase();
  const types = {
    ".html": "text/html",
    ".js": "text/javascript",
    ".mjs": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".json": "application/json",
    ".wasm": "application/wasm",
  };
  return types[ext] || "application/octet-stream";
}

async function startServer(staticRoot) {
  const root = path.resolve(staticRoot);

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, `http://${req.headers.host}`);
    let filePath = path.join(root, url.pathname);
    const stats = await fs.stat(filePath).catch(() => null);
    if (stats?.isDirectory()) {
      filePath = path.join(filePath, "index.html");
    }

    try {
      const data = await fs.readFile(filePath);
      res.writeHead(200, { "Content-Type": guessContentType(filePath) });
      res.end(data);
    } catch {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end("Not found");
    }
  });

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  return { server, url: `http://127.0.0.1:${port}` };
}

function buildArgs(browser, headless, mode, source) {
  if (browser === "chromium") {
    const args = [
      "--autoplay-policy=no-user-gesture-required",
      "--use-fake-ui-for-media-stream",
    ];
    if (mode === "publish" || mode === "both" || source !== "fake") {
      args.push("--use-fake-device-for-media-stream");
    }
    if (headless) {
      args.push("--headless=new");
    }
    if (process.env.CI) {
      args.push("--no-sandbox", "--disable-setuid-sandbox", "--disable-dev-shm-usage");
    }
    return args;
  }
  return [];
}

function buildPlayerUrl(baseUrl, params) {
  const url = new URL("/player.html", baseUrl);
  url.searchParams.set("mode", params.mode);
  if (params.streamId) {
    url.searchParams.set("id", params.streamId);
  }
  if (params.whipUrl) {
    url.searchParams.set("whip_url", params.whipUrl);
  }
  if (params.whepUrl) {
    url.searchParams.set("whep_url", params.whepUrl);
  }
  if (params.vcodec) {
    url.searchParams.set("vcodec", params.vcodec);
  }
  if (params.acodec) {
    url.searchParams.set("acodec", params.acodec);
  }
  if (params.layer) {
    url.searchParams.set("layer", params.layer);
  }
  if (params.token) {
    url.searchParams.set("token", params.token);
  }
  url.searchParams.set("source", params.source);
  url.searchParams.set("timeout", params.timeoutMs.toString());
  return url.toString();
}

function resultPropertyName(mode) {
  if (mode === "publish") return "__LIVE777_PUBLISH_RESULT__";
  if (mode === "subscribe") return "__LIVE777_SUBSCRIBE_RESULT__";
  return "__LIVE777_RESULT__";
}

async function main() {
  const params = parseArgs();
  if (!params.staticRoot) {
    console.error("Missing --static-root");
    process.exit(1);
  }
  if (params.mode !== "publish" && !params.whepUrl) {
    console.error("Missing --whep-url");
    process.exit(1);
  }
  if (
    (params.mode === "publish" || params.mode === "both") &&
    !params.whipUrl
  ) {
    console.error("Missing --whip-url");
    process.exit(1);
  }

  const { server, url } = await startServer(params.staticRoot);
  const playerUrl = buildPlayerUrl(url, params);

  let browserInstance = null;
  let context = null;

  try {
    const launcher = loadPlaywright(params.browser);
    const launchOptions = {
      headless: params.headless,
      args: buildArgs(params.browser, params.headless, params.mode, params.source),
    };
    if (params.channel) {
      launchOptions.channel = params.channel;
    }
    browserInstance = await launcher.launch(launchOptions);
    context = await browserInstance.newContext();
    const page = await context.newPage();

    page.on("console", (msg) => {
      const text = msg.text();
      if (text.startsWith("[")) {
        console.error(`[browser] ${text}`);
      }
    });
    page.on("pageerror", (err) => {
      console.error(`[browser pageerror] ${err.message || err}`);
    });

    await page.goto(playerUrl, { waitUntil: "domcontentloaded" });

    const resultProp = resultPropertyName(params.mode);
    await page.waitForFunction(
      (prop) => window[prop] !== undefined,
      resultProp,
      { timeout: params.timeoutMs + 10000 },
    );

    const value = await page.evaluate((prop) => window[prop], resultProp);
    console.log(JSON.stringify({ mode: params.mode, result: value }));
  } catch (err) {
    console.error(`[runner error] ${err.message || err}`);
    console.log(
      JSON.stringify({
        mode: params.mode,
        result: {
          success: false,
          connected: false,
          error: err.message || String(err),
        },
      }),
    );
  } finally {
    if (context) await context.close().catch(() => {});
    if (browserInstance) await browserInstance.close().catch(() => {});
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((err) => {
  console.error(`[fatal] ${err.message || err}`);
  process.exit(1);
});
