import { WHEPClient } from "@binbat/whip-whep/whep.js";
import { PlayerSurface } from "player-core";
import { createEffect, createSignal, onCleanup } from "solid-js";
import "player-core/style.css";

export default () => {
    const [streamId, setStreamId] = createSignal("");
    const [autoPlay, setAutoPlay] = createSignal(false);
    const [muted, setMuted] = createSignal(false);
    const [controls, setControls] = createSignal(false);
    const [reconnect, setReconnect] = createSignal(0);
    const [token, setToken] = createSignal("");

    const [stream, setStream] = createSignal<MediaStream | null>(null);

    let videoRef: HTMLVideoElement | undefined;
    let peerConnectionRef: RTCPeerConnection | null = null;
    let whepClientRef: WHEPClient | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let disconnectTimer: ReturnType<typeof setTimeout> | undefined;

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

    const clearReconnectTimer = () => {
        if (reconnectTimer) {
            clearTimeout(reconnectTimer);
            reconnectTimer = undefined;
        }
    };

    const clearDisconnectTimer = () => {
        if (disconnectTimer) {
            clearTimeout(disconnectTimer);
            disconnectTimer = undefined;
        }
    };

    const handlePlay = async () => {
        if (whepClientRef || peerConnectionRef) return;
        clearReconnectTimer();

        const pc = new RTCPeerConnection();
        peerConnectionRef = pc;
        pc.addTransceiver("video", { direction: "recvonly" });
        pc.addTransceiver("audio", { direction: "recvonly" });

        const ms = new MediaStream();
        setStream(ms);

        pc.addEventListener("track", (ev: RTCTrackEvent) => {
            if (peerConnectionRef !== pc) return;
            ms.addTrack(ev.track);
            videoRef?.play().catch(() => {
                // Ignore autoplay rejection; click-to-play still works.
            });
        });

        const scheduleDisconnectStop = () => {
            if (disconnectTimer) return;
            disconnectTimer = setTimeout(() => {
                disconnectTimer = undefined;
                if (peerConnectionRef !== pc) return;
                if (
                    pc.iceConnectionState === "disconnected" ||
                    pc.iceConnectionState === "failed" ||
                    pc.iceConnectionState === "closed"
                ) {
                    handleStop();
                }
            }, reconnect() || 3000);
        };

        pc.addEventListener("iceconnectionstatechange", () => {
            switch (pc.iceConnectionState) {
                case "connected":
                case "completed": {
                    clearDisconnectTimer();
                    break;
                }
                case "disconnected": {
                    if (reconnect() > 0) {
                        scheduleDisconnectStop();
                    } else {
                        handleStop();
                    }
                    break;
                }
                case "failed":
                case "closed": {
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
            if (peerConnectionRef === pc) {
                handleStop();
            }
        }
    };

    const handleStop = async (options: { reconnect?: boolean } = {}) => {
        const shouldReconnect = options.reconnect ?? true;
        clearDisconnectTimer();
        setStream(null);
        const whep = whepClientRef;
        const pc = peerConnectionRef;
        whepClientRef = null;
        peerConnectionRef = null;
        if (whep) {
            try {
                await whep.stop();
            } catch {
                pc?.close();
            }
        } else {
            pc?.close();
        }
        if (shouldReconnect && reconnect() > 0) {
            clearReconnectTimer();
            reconnectTimer = setTimeout(() => {
                reconnectTimer = undefined;
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
        clearReconnectTimer();
        handleStop({ reconnect: false });
    });

    return (
        <PlayerSurface
            stream={stream()}
            autoplay={autoPlay()}
            muted={muted()}
            controls={controls()}
            onClick={handleVideoClick}
            onVideoElement={(video) => {
                videoRef = video;
            }}
            getPeerConnection={() => peerConnectionRef}
        />
    );
};
