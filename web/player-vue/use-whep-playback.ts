import {
    type WhepMute,
    WhepPlaybackCore,
    type WhepPlaybackStatus,
} from "player-core";
import {
    type MaybeRefOrGetter,
    onScopeDispose,
    type ShallowRef,
    shallowRef,
    toValue,
    watch,
} from "vue";

export type UseWhepPlaybackOptions = {
    url: MaybeRefOrGetter<string>;
    token?: MaybeRefOrGetter<string>;
    reconnectMs?: MaybeRefOrGetter<number | undefined>;
    createDataChannel?: boolean;
    lowLatency?: MaybeRefOrGetter<boolean>;
    log?: (message: string) => void;
};

export type UseWhepPlayback = {
    stream: ShallowRef<MediaStream | null>;
    peerConnection: ShallowRef<RTCPeerConnection | null>;
    datachannel: ShallowRef<RTCDataChannel | null>;
    status: ShallowRef<WhepPlaybackStatus>;
    error: ShallowRef<Error | null>;
    audioTrackCount: ShallowRef<number>;
    videoTrackCount: ShallowRef<number>;
    play: () => Promise<void>;
    stop: (options?: { reconnect?: boolean }) => Promise<void>;
    mute: (muted: WhepMute) => Promise<void>;
    selectLayer: (layer: string) => Promise<void>;
};

function getter<T>(
    value: MaybeRefOrGetter<T> | undefined,
): (() => T) | undefined {
    return value === undefined ? undefined : () => toValue(value);
}

// Vue adapter over the framework-agnostic `WhepPlaybackCore`: mirrors the
// core state into shallow refs and forwards the imperative methods.
export function useWhepPlayback(
    options: UseWhepPlaybackOptions,
): UseWhepPlayback {
    const core = new WhepPlaybackCore({
        url: () => toValue(options.url),
        token: getter(options.token),
        reconnectMs: getter(options.reconnectMs),
        createDataChannel: options.createDataChannel,
        log: options.log,
    });

    const stream = shallowRef<MediaStream | null>(null);
    const peerConnection = shallowRef<RTCPeerConnection | null>(null);
    const datachannel = shallowRef<RTCDataChannel | null>(null);
    const status = shallowRef<WhepPlaybackStatus>("idle");
    const error = shallowRef<Error | null>(null);
    const audioTrackCount = shallowRef(0);
    const videoTrackCount = shallowRef(0);

    const unsubscribe = core.subscribe((state) => {
        stream.value = state.stream;
        peerConnection.value = state.peerConnection;
        datachannel.value = state.datachannel;
        status.value = state.status;
        error.value = state.error;
        audioTrackCount.value = state.audioTrackCount;
        videoTrackCount.value = state.videoTrackCount;
    });

    // Live-toggle the low-latency hints; receivers that arrive later pick
    // the flag up from the core's `ontrack` handler.
    watch(
        () =>
            options.lowLatency === undefined
                ? false
                : toValue(options.lowLatency),
        (enabled) => core.setLowLatency(enabled),
        { immediate: true },
    );

    onScopeDispose(() => {
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
