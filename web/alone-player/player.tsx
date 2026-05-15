import { WHEPClient } from "@binbat/whip-whep/whep.js";
import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import Stats from "./stats";
import type { StatsNerds } from "./types";
import { collectWebRtcStats } from "./webrtc-stats";
import "./player.css";

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

    const stopSyncStats = () => {
        if (statsInterval) {
            clearInterval(statsInterval);
            statsInterval = null;
        }
        setStatsNerds(null);
    };

    const syncStats = async () => {
        if (!peerConnectionRef) return;

        const stats = await collectWebRtcStats(peerConnectionRef);
        stats.muted = videoRef?.muted;
        setStatsNerds(stats);
    };

    const startSyncStats = () => {
        if (statsInterval) return;
        syncStats();
        statsInterval = setInterval(syncStats, 1000);
    };

    onMount(() => {
        videoRef?.addEventListener("contextmenu", startSyncStats);
    });

    onCleanup(() => {
        handleStop();
        videoRef?.removeEventListener("contextmenu", startSyncStats);
        stopSyncStats();
    });

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
