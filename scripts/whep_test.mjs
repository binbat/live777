#!/usr/bin/env node
/**
 * WHEP browser test — exercise live777's WHEP endpoint with a real Chromium.
 *
 * Usage:
 *   node scripts/whep_test.mjs --stream <name> [options]
 *
 * Options:
 *   --stream <name>       Stream name (required)
 *   --timeout <seconds>   ICE connection timeout (default 10)
 *   --headless            Run Chromium headless (default: headful)
 *   --sdp                 Print offer & answer SDP codec lines
 *   --stats-interval <ms> Stats polling interval (default 1000)
 *   --wait <seconds>      Seconds to collect stats after connected (default 5)
 *
 * Examples:
 *   node scripts/whep_test.mjs --stream h264-test --sdp --wait 8
 *   node scripts/whep_test.mjs --stream vp8-test --headless
 */

import { chromium } from "playwright";
import { parseArgs } from "node:util";

const {
  values: opts,
} = parseArgs({
  options: {
    stream: { type: "string" },
    timeout: { type: "string", default: "10" },
    headless: { type: "boolean", default: false },
    sdp: { type: "boolean", default: false },
    "stats-interval": { type: "string", default: "1000" },
    wait: { type: "string", default: "5" },
  },
});

if (!opts.stream) {
  console.error("Usage: node scripts/whep_test.mjs --stream <name>");
  process.exit(1);
}

const STREAM = opts.stream;
const ICE_TIMEOUT = parseInt(opts.timeout, 10) * 1000;
const STATS_INTERVAL = parseInt(opts["stats-interval"], 10);
const WAIT_SEC = parseInt(opts.wait, 10);

console.log(`Testing WHEP: stream=${STREAM} headless=${opts.headless}`);

const browser = await chromium.launch({
  headless: opts.headless,
  args: [
    "--disable-features=PrivateNetworkAccessForNavigations,PrivateNetworkAccessRespectPreflightResults,PrivateNetworkAccessSendPreflights",
    "--disable-web-security",
  ],
});

const page = await browser.newPage();
page.on("console", (msg) => console.log("BROWSER:", msg.text()));
await page.goto("about:blank");

const result = await page.evaluate(
  async ({ stream, iceTimeout, statsInterval, waitSec, printSdp }) => {
    const log = [];
    const addLog = (msg) => {
      log.push(msg);
      console.log(msg);
    };

    try {
      const pc = new RTCPeerConnection({
        iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
      });
      pc.addTransceiver("video", { direction: "recvonly" });

      // --- stats collector ---
      const statsHistory = [];
      let statsTimer = 0;
      const startStats = () => {
        statsTimer = setInterval(async () => {
          const stats = await pc.getStats();
          let inbound = null;
          for (const [, s] of stats) {
            if (s.type === "inbound-rtp" && s.kind === "video") {
              inbound = {
                pkts: s.packetsReceived,
                frames: s.framesDecoded,
                fir: s.firCount,
                pli: s.pliCount,
                nack: s.nackCount,
                mime: s.mimeType,
                bytes: s.bytesReceived,
              };
            }
            if (s.type === "codec") {
              addLog(
                `CODEC: pt=${s.payloadType} mime=${s.mimeType} fmtp=${s.sdpFmtpLine}`,
              );
            }
          }
          if (inbound) {
            statsHistory.push(inbound);
            addLog(
              `STATS: pkts=${inbound.pkts} frames=${inbound.frames} mime=${inbound.mime} pli=${inbound.pli} nack=${inbound.nack}`,
            );
          }
        }, statsInterval);
      };

      // --- offer / answer ---
      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      const offerSdp = pc.localDescription.sdp;

      if (printSdp) {
        offerSdp.split("\r\n").forEach((l) => {
          if (
            (l.includes("H264") || l.includes("H265")) &&
            !l.includes("rtx")
          ) {
            addLog("OFFER: " + l);
          }
        });
      }

      const resp = await fetch(`http://localhost:7777/whep/${stream}`, {
        method: "POST",
        headers: { "Content-Type": "application/sdp" },
        body: offerSdp,
      });

      if (!resp.ok) {
        addLog(`WHEP HTTP ${resp.status} ${resp.statusText}`);
        return { success: false, error: `HTTP ${resp.status}`, log };
      }

      const answerSdp = await resp.text();

      if (printSdp) {
        answerSdp.split("\r\n").forEach((l) => {
          if (
            (l.includes("H264") || l.includes("H265") || l.includes("sprop") ||
              l.includes("profile-level") || l.includes("packetization")) &&
            !l.includes("rtx")
          ) {
            addLog("ANSWER: " + l);
          }
        });
        // Print m=video line
        const mVideo = answerSdp
          .split("\r\n")
          .filter(
            (l, i, arr) =>
              l.startsWith("m=") && l.includes("video"),
          );
        addLog("ANSWER m=video: " + (mVideo[0] || "<none>"));
      }

      await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });

      // --- connect ---
      addLog("Waiting for ICE connection...");

      pc.ontrack = (e) => addLog("onTrack: kind=" + e.track.kind);

      startStats();

      const connected = await new Promise((resolve) => {
        let resolved = false;
        const t = setTimeout(() => {
          if (!resolved) {
            resolved = true;
            resolve(false);
          }
        }, iceTimeout);
        pc.oniceconnectionstatechange = () => {
          addLog("ICE: " + pc.iceConnectionState);
          if (pc.iceConnectionState === "connected" && !resolved) {
            resolved = true;
            clearTimeout(t);
            resolve(true);
          }
          if (pc.iceConnectionState === "failed" && !resolved) {
            resolved = true;
            clearTimeout(t);
            resolve(false);
          }
        };
      });

      if (!connected) {
        clearInterval(statsTimer);
        addLog("ICE timeout / failed");
        pc.close();
        return {
          success: false,
          error: "ICE " + (pc.iceConnectionState || "timeout"),
          statsHistory,
          log,
        };
      }

      addLog(`Connected, collecting stats for ${waitSec}s...`);
      await new Promise((r) => setTimeout(r, waitSec * 1000));
      clearInterval(statsTimer);

      // --- final summary ---
      const finalStats = await pc.getStats();
      const allCodecs = [];
      let finalInbound = null;
      for (const [, s] of finalStats) {
        if (s.type === "codec") {
          allCodecs.push({
            pt: s.payloadType,
            mime: s.mimeType,
            fmtp: s.sdpFmtpLine,
          });
        }
        if (s.type === "inbound-rtp" && s.kind === "video") {
          finalInbound = {
            pkts: s.packetsReceived,
            frames: s.framesDecoded,
            fir: s.firCount,
            pli: s.pliCount,
            nack: s.nackCount,
            bytes: s.bytesReceived,
            mime: s.mimeType,
          };
        }
      }

      pc.close();

      return {
        success: true,
        codecs: allCodecs,
        finalInbound,
        statsHistory,
        log,
      };
    } catch (e) {
      addLog("ERR: " + e.message);
      return { success: false, error: e.message, log };
    }
  },
  {
    stream: STREAM,
    iceTimeout: ICE_TIMEOUT,
    statsInterval: STATS_INTERVAL,
    waitSec: WAIT_SEC,
    printSdp: opts.sdp,
  },
);

console.log(JSON.stringify(result, null, 2));
await browser.close();
