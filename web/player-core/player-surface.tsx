import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import StatsForNerds from "./stats";
import type { StatsNerds } from "./types";
import { collectWebRtcStats } from "./webrtc-stats";

interface Props {
    stream?: MediaStream | null;
    autoplay?: boolean;
    muted?: boolean;
    controls?: boolean;
    onClick?: (event: MouseEvent) => void;
    onVideoElement?: (video: HTMLVideoElement) => void;
    getPeerConnection?: () => RTCPeerConnection | null;
}

export default function PlayerSurface(props: Props) {
    const [statsNerds, setStatsNerds] = createSignal<StatsNerds | null>(null);

    let ref: HTMLVideoElement | undefined;
    let statsInterval: ReturnType<typeof setInterval> | null = null;
    let detachStreamListeners: (() => void) | undefined;

    const play = () => {
        if (!props.autoplay || !ref || !ref.srcObject) return;
        ref.muted = !!props.muted;
        ref.play().catch(() => {
            // Autoplay can be blocked if the caller does not mute the video.
        });
    };

    createEffect(() => {
        if (ref) {
            ref.srcObject = props.stream ?? null;
            queueMicrotask(play);
        }
    });

    createEffect(() => {
        detachStreamListeners?.();
        detachStreamListeners = undefined;
        if (!ref || !props.stream) return;

        const currentRef = ref;
        currentRef.addEventListener("loadedmetadata", play);
        currentRef.addEventListener("canplay", play);
        detachStreamListeners = () => {
            currentRef.removeEventListener("loadedmetadata", play);
            currentRef.removeEventListener("canplay", play);
        };
        play();
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

    onMount(() => {
        ref?.addEventListener("contextmenu", startSyncStats);
    });

    onCleanup(() => {
        detachStreamListeners?.();
        ref?.removeEventListener("contextmenu", startSyncStats);
        stopSyncStats();
    });

    return (
        <div id="player" class="player-wrapper">
            <video
                ref={ref}
                autoplay={props.autoplay}
                muted={props.muted}
                controls={props.controls}
                onClick={props.onClick}
                playsinline
            />
            <Show when={statsNerds()}>
                {(stats) => (
                    <div class="stats-container" id="stats">
                        <StatsForNerds
                            stats={stats()}
                            onClose={stopSyncStats}
                        />
                    </div>
                )}
            </Show>
        </div>
    );
}
