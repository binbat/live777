import { type Accessor, createEffect, createSignal, onCleanup } from "solid-js";
import {
    type WhepMute,
    WhepPlaybackCore,
    type WhepPlaybackStatus,
} from "./whep-core";

export type { WhepMute, WhepPlaybackStatus };

type MaybeAccessor<T> = T | Accessor<T>;

export type WhepPlaybackOptions = {
    url: Accessor<string>;
    token?: Accessor<string>;
    reconnectMs?: Accessor<number>;
    createDataChannel?: boolean;
    // When true, drop the receiver jitter buffer to zero so frames render as
    // soon as they are decoded. This is required to reach high frame rates
    // (e.g. 120fps) at the cost of smoothness on jittery networks.
    lowLatency?: Accessor<boolean>;
    log?: (message: string) => void;
};

export type WhepPlayback = {
    stream: Accessor<MediaStream | null>;
    peerConnection: Accessor<RTCPeerConnection | null>;
    datachannel: Accessor<RTCDataChannel | null>;
    status: Accessor<WhepPlaybackStatus>;
    error: Accessor<Error | null>;
    audioTrackCount: Accessor<number>;
    videoTrackCount: Accessor<number>;
    play: () => Promise<void>;
    stop: (options?: { reconnect?: boolean }) => Promise<void>;
    mute: (muted: WhepMute) => Promise<void>;
    selectLayer: (layer: string) => Promise<void>;
};

function resolve<T>(value: MaybeAccessor<T> | undefined, fallback: T): T {
    if (typeof value === "function") {
        return (value as Accessor<T>)();
    }
    return value ?? fallback;
}

// Solid adapter over the framework-agnostic `WhepPlaybackCore`: mirrors the
// core state into signals and forwards the imperative methods.
export function createWhepPlayback(options: WhepPlaybackOptions): WhepPlayback {
    const core = new WhepPlaybackCore({
        url: options.url,
        token: options.token,
        reconnectMs: options.reconnectMs,
        createDataChannel: options.createDataChannel,
        log: options.log,
    });

    const [stream, setStream] = createSignal<MediaStream | null>(null);
    const [peerConnection, setPeerConnection] =
        createSignal<RTCPeerConnection | null>(null);
    const [datachannel, setDatachannel] = createSignal<RTCDataChannel | null>(
        null,
    );
    const [status, setStatus] = createSignal<WhepPlaybackStatus>("idle");
    const [error, setError] = createSignal<Error | null>(null);
    const [audioTrackCount, setAudioTrackCount] = createSignal(0);
    const [videoTrackCount, setVideoTrackCount] = createSignal(0);

    const unsubscribe = core.subscribe((state) => {
        setStream(state.stream);
        setPeerConnection(state.peerConnection);
        setDatachannel(state.datachannel);
        setStatus(state.status);
        setError(state.error);
        setAudioTrackCount(state.audioTrackCount);
        setVideoTrackCount(state.videoTrackCount);
    });

    // Live-toggle the low-latency hints on every receiver of the active
    // peer connection. Receivers that arrive later pick the flag up from
    // the core's `ontrack` handler.
    createEffect(() => {
        core.setLowLatency(resolve(options.lowLatency, false));
    });

    onCleanup(() => {
        unsubscribe();
        void core.destroy();
    });

    return {
        stream,
        peerConnection,
        datachannel,
        status,
        error,
        audioTrackCount,
        videoTrackCount,
        play: () => core.play(),
        stop: (stopOptions) => core.stop(stopOptions),
        mute: (muted) => core.mute(muted),
        selectLayer: (layer) => core.selectLayer(layer),
    };
}
