import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import Stats from "../../alone-player/stats";
import type { StatsNerds } from "../../alone-player/types";
import { collectWebRtcStats } from "../../alone-player/webrtc-stats";
import "../../alone-player/player.css";

const DisplayWidthOptions = [
    { value: "320px", text: "320px" },
    { value: "480px", text: "480px" },
    { value: "600px", text: "600px" },
    { value: "1280px", text: "1280px" },
    { value: "1920px", text: "1920px" },
    { value: "", text: "auto" },
];

export default function Player(props: {
    stream: MediaStream;
    onVideoElement?: (video: HTMLVideoElement) => void;
    getPeerConnection?: () => RTCPeerConnection | null;
}) {
    const [resolution, setResolution] = createSignal("");
    const [displayWidth, setDisplayWidth] = createSignal("320px");
    const [statsNerds, setStatsNerds] = createSignal<StatsNerds | null>(null);

    let ref: HTMLVideoElement | undefined;
    let statsInterval: ReturnType<typeof setInterval> | null = null;

    createEffect(() => {
        if (ref) {
            ref.srcObject = props.stream;
        }
    });

    createEffect(() => {
        if (ref) {
            props.onVideoElement?.(ref);
        }
    });

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
        stats.muted = ref?.muted;
        setStatsNerds(stats);
    };

    const startSyncStats = () => {
        if (statsInterval) return;
        syncStats();
        statsInterval = setInterval(syncStats, 1000);
    };

    const handleResize = () => {
        if (!ref) return;
        setResolution(`${ref.videoWidth}x${ref.videoHeight}`);
    };

    onMount(() => {
        ref?.addEventListener("contextmenu", startSyncStats);
        ref?.addEventListener("resize", handleResize);
    });

    onCleanup(() => {
        ref?.removeEventListener("contextmenu", startSyncStats);
        ref?.removeEventListener("resize", handleResize);
        stopSyncStats();
    });

    return (
        <>
            <h5>Raw Resolution: {resolution()}</h5>
            <label>
                Video Width:
                <select
                    value={displayWidth()}
                    onChange={(e) => {
                        setDisplayWidth(e.target.value);
                    }}
                >
                    {DisplayWidthOptions.map((o) => (
                        <option value={o.value}>{o.text}</option>
                    ))}
                </select>
            </label>
            <br />
            <div style={{ width: displayWidth(), margin: "0 auto" }}>
                <div id="player" class="player-wrapper">
                    <video ref={ref} autoplay muted controls />
                    <Show when={statsNerds()}>
                        {(stats) => (
                            <div class="stats-container" id="stats">
                                <Stats
                                    stats={stats()}
                                    onClose={stopSyncStats}
                                />
                            </div>
                        )}
                    </Show>
                </div>
            </div>
        </>
    );
}
