import { WHEPClient } from "@binbat/whip-whep/whep.js";
import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import type { StatsNerds } from "./types";
import "./player.css";
import Stats from "./stats";

export default () => {
    const [streamId, setStreamId] = createSignal("");
    const [autoPlay, setAutoPlay] = createSignal(false);
    const [muted, setMuted] = createSignal(false);
    const [controls, setControls] = createSignal(false);
    const [reconnect, setReconnect] = createSignal(0);
    const [token, setToken] = createSignal("");

    const [statsNerds, setStatsNerds] = createSignal<StatsNerds | null>(null);

    let videoRef: HTMLVideoElement | undefined;
    let peerConnectionRef: RTCPeerConnection | null = null;
    let whepClientRef: WHEPClient | null = null;

    let statsInterval: ReturnType<typeof setInterval> | null = null;

    createEffect(() => {
        const params = new URLSearchParams(location.search);
        setStreamId(params.get("id") ?? "");
        setAutoPlay(params.has("autoplay"));
        setControls(params.has("controls"));
        setMuted(params.has("muted"));
        const n = Number.parseInt(params.get("reconnect") ?? "0", 10);
        setReconnect(Number.isNaN(n) ? 0 : n);
        setToken(params.get("token") ?? "");
    });

    createEffect(() => {
        if (!streamId() || !autoPlay()) return;
        handlePlay();
    });

    const handlePlay = async () => {
        const pc = new RTCPeerConnection();
        peerConnectionRef = pc;
        pc.addTransceiver("video", { direction: "recvonly" });
        pc.addTransceiver("audio", { direction: "recvonly" });

        const ms = new MediaStream();

        if (videoRef) {
            videoRef.srcObject = ms;
        }

        pc.addEventListener("track", (ev: RTCTrackEvent) => {
            ms.addTrack(ev.track);
        });

        pc.addEventListener("iceconnectionstatechange", () => {
            switch (pc.iceConnectionState) {
                case "disconnected": {
                    handleStop();
                    break;
                }
            }
        });

        const whep = new WHEPClient();
        whepClientRef = whep;
        const url = `${location.origin}/whep/${streamId()}`;

        try {
            await whep.view(pc, url, token());
        } catch {
            handleStop();
        }
    };

    const handleStop = async () => {
        if (videoRef) {
            videoRef.srcObject = null;
        }
        if (whepClientRef) {
            await whepClientRef.stop();
            whepClientRef = null;
        }
        if (peerConnectionRef) {
            peerConnectionRef = null;
        }
        if (reconnect() > 0) {
            setTimeout(() => {
                handleReconnect();
            }, reconnect());
        }
    };

    const handleReconnect = async () => {
        await handlePlay();
        videoRef?.play();
    };

    const handleVideoClick = async (e: MouseEvent) => {
        if (!whepClientRef) {
            e.preventDefault();
            await handlePlay();
            videoRef?.play();
        }
    };

    onCleanup(() => {
        handleStop();
        statsInterval && clearInterval(statsInterval);
    });

    onMount(() => {
        videoRef?.addEventListener("contextmenu", startSyncStats);
    });

    onCleanup(() => {
        videoRef?.removeEventListener("contextmenu", startSyncStats);
    });

    function startSyncStats() {
        statsInterval = setInterval(async () => {
            if (!peerConnectionRef) return;

            const tmpStats: StatsNerds = {
                bytesReceived: 0,
                bytesSent: 0,
                currentRoundTripTime: 0,
            };

            const stats = await peerConnectionRef?.getStats();
            stats.forEach((report) => {
                if (report.type === "transport") {
                    tmpStats.bytesReceived = report.bytesReceived ?? 0;
                    tmpStats.bytesSent = report.bytesSent ?? 0;
                } else if (report.type === "codec") {
                    const [kind, codec] = report.mimeType
                        .toLowerCase()
                        .split("/");
                    if (kind === "video") {
                        tmpStats.vcodec = `${codec}@${report.sdpFmtpLine ?? ""}`;
                    } else if (kind === "audio") {
                        tmpStats.acodec = `${codec}@${report.sdpFmtpLine ?? ""}`;
                    } else {
                        console.log("Unknown mimeType", report.mimeType);
                    }
                } else if (
                    report.type === "candidate-pair" &&
                    report.nominated
                ) {
                    tmpStats.currentRoundTripTime =
                        report.currentRoundTripTime ?? 0;
                }

                if (report.type === "inbound-rtp" && report.kind === "video") {
                    tmpStats.frameWidth = report.frameWidth;
                    tmpStats.frameHeight = report.frameHeight;
                    tmpStats.framesPerSecond = report.framesPerSecond;
                }

                if (report.type === "inbound-rtp" && report.kind === "audio") {
                    tmpStats.audioLevel = report.audioLevel;
                }
            });
            setStatsNerds(tmpStats);
        }, 1000);
    }

    function stopSyncStats() {
        if (statsInterval) {
            clearInterval(statsInterval);
        }
        setStatsNerds(null);
    }

    return (
        <div id="player" class="player-wrapper">
            <video
                ref={videoRef}
                autoplay={autoPlay()}
                muted={muted()}
                controls={controls()}
                onClick={handleVideoClick}
            />
            <Show when={statsNerds()}>
                {(stats) => (
                    <div class="stats-container" id="stats">
                        <Stats stats={stats()} onClose={stopSyncStats} />
                    </div>
                )}
            </Show>
        </div>
    );
};
