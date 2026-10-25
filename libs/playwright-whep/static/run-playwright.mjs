// Playwright runner for the Live777 minimal WebRTC test player.
// This script is invoked from Rust; see libs/playwright-whep/src/lib.rs.

import fs from "node:fs/promises";
import http from "node:http";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

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
        diagnosticsDir: "",
        trace: false,
    };
    for (let i = 0; i < args.length; i += 2) {
        if (i + 1 >= args.length) {
            console.error(`Missing value for argument ${args[i]}`);
            process.exit(1);
        }
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
            case "--diagnostics-dir":
                result.diagnosticsDir = value;
                break;
            case "--trace":
                result.trace = value === "true";
                break;
        }
    }
    return result;
}

function loadPlaywright(browserName) {
    const modulePath = process.env.PLAYWRIGHT_MODULE_PATH;
    if (!modulePath) {
        throw new Error(
            "PLAYWRIGHT_MODULE_PATH environment variable is not set",
        );
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
        let filePath = path.resolve(path.join(root, url.pathname));

        // Reject requests that escape the served directory.
        if (!filePath.startsWith(root + path.sep) && filePath !== root) {
            res.writeHead(403, { "Content-Type": "text/plain" });
            res.end("Forbidden");
            return;
        }

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
            args.push(
                "--no-sandbox",
                "--disable-setuid-sandbox",
                "--disable-dev-shm-usage",
            );
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
    // Token is passed via an in-page message instead of the URL to avoid
    // leaking it into browser history, server logs, and referrer headers.
    url.searchParams.set("source", params.source);
    url.searchParams.set("timeout", params.timeoutMs.toString());
    return url.toString();
}

function resultPropertyName(mode) {
    if (mode === "publish") return "__LIVE777_PUBLISH_RESULT__";
    if (mode === "subscribe") return "__LIVE777_SUBSCRIBE_RESULT__";
    return "__LIVE777_RESULT__";
}

function emptyDiagnostics() {
    return {
        stateTimeline: [],
        candidateTypeCounts: {},
        selectedPair: null,
        rtp: [],
    };
}

function runnerFailure(mode, error) {
    const result = { success: false, connected: false, error };
    if (mode === "both") {
        return { success: false, publish: null, subscribe: null, error };
    }
    return result;
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

    const diagnosticsDir = params.diagnosticsDir
        ? path.resolve(params.diagnosticsDir)
        : null;
    const artifacts = {
        directory: diagnosticsDir || "",
        retained: false,
        capture_errors: [],
    };
    const artifactPath = (name) =>
        diagnosticsDir && path.join(diagnosticsDir, name);
    const redact = (value) => {
        let text = String(value);
        if (params.token) text = text.split(params.token).join("[REDACTED]");
        return text;
    };
    const persistCaptureErrors = async () => {
        const file = artifactPath("capture-errors.json");
        if (file) {
            await fs.writeFile(
                file,
                `${JSON.stringify(artifacts.capture_errors, null, 2)}\n`,
                "utf8",
            );
        }
    };
    const writeLog = async (kind, value) => {
        const file = artifactPath("console.log");
        if (!file) return;
        const line = `[${new Date().toISOString()}] [${kind}] ${redact(value)}\n`;
        try {
            await fs.appendFile(file, line, "utf8");
            artifacts.console_log = file;
        } catch (err) {
            const message = `console capture failed: ${err.message || err}`;
            if (!artifacts.capture_errors.includes(message)) {
                artifacts.capture_errors.push(message);
                await persistCaptureErrors().catch(() => {});
            }
        }
    };
    const capture = async (name, operation) => {
        try {
            await operation();
        } catch (err) {
            const message = `${name} capture failed: ${redact(err.message || err)}`;
            artifacts.capture_errors.push(message);
            await persistCaptureErrors().catch(() => {});
            await writeLog("runner", message);
        }
    };

    let server = null;
    let browserInstance = null;
    let context = null;
    let page = null;
    let result = null;

    if (diagnosticsDir) {
        await fs.mkdir(diagnosticsDir, { recursive: true });
        await writeLog("runner", `Starting ${params.mode} browser run`);
    }

    let rejectTermination;
    const termination = new Promise((_, reject) => {
        rejectTermination = reject;
    });
    const onSigterm = () =>
        rejectTermination(new Error("Playwright runner received SIGTERM"));
    const onSigint = () =>
        rejectTermination(new Error("Playwright runner received SIGINT"));
    process.once("SIGTERM", onSigterm);
    process.once("SIGINT", onSigint);

    try {
        await Promise.race([
            (async () => {
                const started = await startServer(params.staticRoot);
                server = started.server;
                const playerUrl = buildPlayerUrl(started.url, params);
                const launcher = loadPlaywright(params.browser);
                const launchOptions = {
                    headless: params.headless,
                    args: buildArgs(
                        params.browser,
                        params.headless,
                        params.mode,
                        params.source,
                    ),
                };
                if (params.channel) {
                    launchOptions.channel = params.channel;
                } else if (params.browser === "chromium") {
                    launchOptions.channel = "chromium";
                }
                browserInstance = await launcher.launch(launchOptions);
                context = await browserInstance.newContext();
                if (params.trace && diagnosticsDir) {
                    await capture("trace start", () =>
                        context.tracing.start({
                            screenshots: true,
                            snapshots: true,
                            sources: true,
                        }),
                    );
                }
                page = await context.newPage();

                page.on("console", (msg) => {
                    const text = msg.text();
                    void writeLog(`console:${msg.type()}`, text);
                    if (text.startsWith("[")) {
                        console.error(`[browser] ${redact(text)}`);
                    }
                });
                page.on("pageerror", (err) => {
                    void writeLog("pageerror", err.message || err);
                    console.error(
                        `[browser pageerror] ${redact(err.message || err)}`,
                    );
                });

                await page.goto(playerUrl, { waitUntil: "domcontentloaded" });

                if (params.token) {
                    await page.evaluate((token) => {
                        window.__LIVE777_TOKEN__ = token;
                    }, params.token);
                }

                const resultProp = resultPropertyName(params.mode);
                await page.waitForFunction(
                    (prop) => window[prop] !== undefined,
                    resultProp,
                    { timeout: params.timeoutMs + 10000 },
                );

                result = await page.evaluate(
                    (prop) => window[prop],
                    resultProp,
                );
            })(),
            termination,
        ]);
    } catch (err) {
        const message = redact(err.message || err);
        await writeLog("runner", message);
        console.error(`[runner error] ${message}`);
        result = runnerFailure(params.mode, message);
    } finally {
        artifacts.retained = Boolean(diagnosticsDir && !result?.success);
        if (diagnosticsDir) {
            await capture("sdp-ice", async () => {
                const diagnostics = page
                    ? await page.evaluate(
                          () => window.__LIVE777_DIAGNOSTICS__ || null,
                      )
                    : null;
                const file = artifactPath("sdp-ice.json");
                await fs.writeFile(
                    file,
                    `${JSON.stringify(diagnostics || emptyDiagnostics(), null, 2)}\n`,
                    "utf8",
                );
                artifacts.sdp_ice = file;
            });
        }
        if (artifacts.retained && page) {
            await capture("screenshot", async () => {
                const file = artifactPath("screenshot.png");
                await page.screenshot({ path: file, fullPage: true });
                artifacts.screenshot = file;
            });
        } else if (artifacts.retained) {
            artifacts.capture_errors.push(
                "screenshot capture failed: browser page was not created",
            );
        }
        if (context && params.trace && diagnosticsDir) {
            if (artifacts.retained) {
                await capture("trace", async () => {
                    const file = artifactPath("trace.zip");
                    await context.tracing.stop({ path: file });
                    artifacts.trace = file;
                });
            } else {
                await capture("trace cleanup", () => context.tracing.stop());
            }
        }
        if (context) {
            await capture("browser context cleanup", () => context.close());
        }
        if (browserInstance) {
            await capture("browser cleanup", () => browserInstance.close());
        }
        if (server) await new Promise((resolve) => server.close(resolve));
        process.removeListener("SIGTERM", onSigterm);
        process.removeListener("SIGINT", onSigint);
        if (diagnosticsDir) await persistCaptureErrors().catch(() => {});
    }

    const output = { mode: params.mode, result };
    if (diagnosticsDir) output.artifacts = artifacts;
    console.log(JSON.stringify(output));
}

main().catch((err) => {
    console.error(`[fatal] ${err.message || err}`);
    process.exit(1);
});
