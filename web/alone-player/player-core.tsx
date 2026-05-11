import { Show, createSignal, onCleanup, onMount } from "solid-js";
import type { StatsNerds } from "./types";
import Stats from "./stats";
import { collectWebRtcStats } from "./webrtc-stats";
import "./player.css";

interface PlayerCoreProps {
    autoplay?: boolean;
    controls?: boolean;
    muted?: boolean;
    onClick?: (e: MouseEvent) => void | Promise<void>;
    onVideoElement?: (video: HTMLVideoElement) => void;
    getPeerConnection?: () => RTCPeerConnection | null;
}

export default function PlayerCore(props: PlayerCoreProps) {
    const [statsNerds, setStatsNerds] = createSignal<StatsNerds | null>(null);

    let videoRef: HTMLVideoElement | undefined;
    let statsInterval: ReturnType<typeof setInterval> | null = null;

    const stopSyncStats = () => {
        if (statsInterval) {
            clearInterval(statsInterval);
            statsInterval = null;
        }
        setStatsNerds(null);
    };

    const syncStats = async () => {
        const peerConnection = props.getPeerConnection?.();
        if (!peerConnection) return;

        const stats = await collectWebRtcStats(peerConnection);
        stats.muted = videoRef?.muted;
        setStatsNerds(stats);
    };

    const startSyncStats = () => {
        if (statsInterval) return;
        syncStats();
        statsInterval = setInterval(syncStats, 1000);
    };

    onMount(() => {
        if (!videoRef) return;
        props.onVideoElement?.(videoRef);
        videoRef.addEventListener("contextmenu", startSyncStats);
    });

    onCleanup(() => {
        videoRef?.removeEventListener("contextmenu", startSyncStats);
        stopSyncStats();
    });

    return (
        <div id="player" class="player-wrapper">
            <video
                ref={videoRef}
                autoplay={props.autoplay}
                muted={props.muted}
                controls={props.controls}
                onClick={props.onClick}
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
}
