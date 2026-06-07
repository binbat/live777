import { createWhepPlayback, PlayerSurface } from "player-core";
import { createEffect, createSignal, onCleanup } from "solid-js";
import "player-core/style.css";
import "./style.css";

export default () => {
    const [streamId, setStreamId] = createSignal("");
    const [autoPlay, setAutoPlay] = createSignal(false);
    const [muted, setMuted] = createSignal(false);
    const [controls, setControls] = createSignal(false);
    const [reconnect, setReconnect] = createSignal(0);
    const [token, setToken] = createSignal("");

    const playback = createWhepPlayback({
        url: () => `${location.origin}/whep/${streamId()}`,
        token,
        reconnectMs: reconnect,
        log: () => {},
    });

    let videoRef: HTMLVideoElement | undefined;

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
        void playback.play();
    });

    createEffect(() => {
        if (playback.status() !== "playing" || !playback.stream()) return;
        videoRef?.play().catch(() => {
            // Ignore autoplay rejection; click-to-play still works.
        });
    });

    const handleVideoClick = async (e: MouseEvent) => {
        if (playback.peerConnection()) return;
        e.preventDefault();
        await playback.play();
        videoRef?.play().catch(() => {
            // Ignore autoplay rejection; click-to-play still works.
        });
    };

    onCleanup(() => {
        void playback.stop({ reconnect: false });
    });

    return (
        <PlayerSurface
            stream={playback.stream()}
            autoplay={autoPlay()}
            muted={muted()}
            controls={controls()}
            onClick={handleVideoClick}
            onVideoElement={(video) => {
                videoRef = video;
            }}
            getPeerConnection={() => playback.peerConnection()}
        />
    );
};
