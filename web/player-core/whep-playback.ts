import { WHEPClient } from "@binbat/whip-whep/whep.js";
import { type Accessor, createSignal, onCleanup } from "solid-js";

type MaybeAccessor<T> = T | Accessor<T>;

export type WhepMute = {
    kind: "audio" | "video";
    enabled: boolean;
};

export type WhepPlaybackStatus =
    | "idle"
    | "connecting"
    | "playing"
    | "reconnecting"
    | "stopped"
    | "error";

export type WhepPlaybackOptions = {
    url: Accessor<string>;
    token?: Accessor<string>;
    reconnectMs?: Accessor<number>;
    createDataChannel?: boolean;
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

function is404Error(error: unknown) {
    const maybe = error as { response?: { status?: number }; status?: number };
    const status = maybe?.response?.status ?? maybe?.status;
    return status === 404 || String(error).includes("404");
}

export function createWhepPlayback(options: WhepPlaybackOptions): WhepPlayback {
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

    let whepClient: WHEPClient | null = null;
    let activePeerConnection: RTCPeerConnection | null = null;
    let playToken = 0;
    let disconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;

    const log = options.log ?? (() => {});

    const clearReconnectTimer = () => {
        if (reconnectTimer !== undefined) {
            clearTimeout(reconnectTimer);
            reconnectTimer = undefined;
        }
    };

    const clearDisconnectTimer = () => {
        if (disconnectTimer !== undefined) {
            clearTimeout(disconnectTimer);
            disconnectTimer = undefined;
        }
    };

    const resetState = () => {
        setStream(null);
        setPeerConnection(null);
        setDatachannel(null);
        setAudioTrackCount(0);
        setVideoTrackCount(0);
    };

    const cleanupPeerConnection = (pc: RTCPeerConnection | null) => {
        if (!pc) return;
        pc.ontrack = null;
        pc.oniceconnectionstatechange = null;
        pc.onconnectionstatechange = null;
        try {
            pc.close();
        } catch {
            // Ignore close errors during teardown.
        }
    };

    const cleanupClient = async (client: WHEPClient | null) => {
        if (!client) return;
        try {
            await client.stop();
        } catch (stopError) {
            if (!is404Error(stopError)) {
                log(`stop error: ${String(stopError)}`);
            }
        }
    };

    const stop = async (opts: { reconnect?: boolean } = {}) => {
        const shouldReconnect = opts.reconnect ?? false;
        const reconnectDelay = resolve(options.reconnectMs, 0);
        const shouldScheduleReconnect = shouldReconnect && reconnectDelay > 0;
        const client = whepClient;
        const pc = activePeerConnection;
        whepClient = null;
        activePeerConnection = null;
        playToken += 1;
        clearDisconnectTimer();
        clearReconnectTimer();
        resetState();
        setStatus(shouldScheduleReconnect ? "reconnecting" : "stopped");

        await cleanupClient(client);
        cleanupPeerConnection(pc);

        if (!shouldScheduleReconnect) {
            return;
        }

        reconnectTimer = setTimeout(() => {
            reconnectTimer = undefined;
            void play();
        }, reconnectDelay);
    };

    const play = async () => {
        if (peerConnection()) return;

        clearDisconnectTimer();
        clearReconnectTimer();
        setStatus("connecting");
        setError(null);

        const pc = new RTCPeerConnection();
        const ms = new MediaStream();
        const client = new WHEPClient();
        const nextPlayToken = playToken + 1;
        playToken = nextPlayToken;

        whepClient = client;
        activePeerConnection = pc;
        setPeerConnection(pc);
        setStream(ms);

        if (options.createDataChannel) {
            setDatachannel(pc.createDataChannel(""));
        }

        pc.addTransceiver("video", { direction: "recvonly" });
        pc.addTransceiver("audio", { direction: "recvonly" });

        const syncTrackCounts = () => {
            setAudioTrackCount(ms.getAudioTracks().length);
            setVideoTrackCount(ms.getVideoTracks().length);
        };

        pc.ontrack = (event) => {
            if (playToken !== nextPlayToken) return;
            log(`track: ${event.track.kind}`);
            ms.addTrack(event.track);
            syncTrackCounts();
            setStream(ms);
        };

        pc.oniceconnectionstatechange = () => {
            if (playToken !== nextPlayToken) return;
            log(`ICE State: ${pc.iceConnectionState}`);
            if (
                pc.iceConnectionState === "connected" ||
                pc.iceConnectionState === "completed"
            ) {
                clearDisconnectTimer();
                setStatus("playing");
                return;
            }
            if (pc.iceConnectionState === "disconnected") {
                const reconnectMs = options.reconnectMs?.();
                const disconnectDelay =
                    reconnectMs === undefined ? 3000 : reconnectMs;
                const shouldReconnect =
                    reconnectMs !== undefined && reconnectMs > 0;

                clearDisconnectTimer();

                if (disconnectDelay <= 0) {
                    void stop({ reconnect: false });
                    return;
                }

                setStatus("reconnecting");
                disconnectTimer = setTimeout(() => {
                    disconnectTimer = undefined;
                    void stop({ reconnect: shouldReconnect });
                }, disconnectDelay);
                return;
            }
            if (pc.iceConnectionState === "failed") {
                void stop({
                    reconnect: resolve(options.reconnectMs, 0) > 0,
                });
                return;
            }
            if (pc.iceConnectionState === "closed") {
                void stop({
                    reconnect: resolve(options.reconnectMs, 0) > 0,
                });
            }
        };

        pc.onconnectionstatechange = () => {
            if (playToken !== nextPlayToken) return;
            log(`connection State: ${pc.connectionState}`);
        };

        try {
            log("http begined");
            await client.view(pc, options.url(), resolve(options.token, ""));
            if (playToken === nextPlayToken) {
                syncTrackCounts();
                setStatus("playing");
            }
        } catch (playError) {
            if (playToken !== nextPlayToken) return;
            const err =
                playError instanceof Error
                    ? playError
                    : new Error(String(playError));
            setError(err);
            setStatus("error");
            log(`ERROR: ${String(playError)}`);
            await stop({
                reconnect: resolve(options.reconnectMs, 0) > 0,
            });
        }
    };

    const mute = async (muted: WhepMute) => {
        if (!whepClient) return;
        log(`mute: ${JSON.stringify(muted)}`);
        await whepClient.mute(muted);
    };

    const selectLayer = async (layer: string) => {
        if (!whepClient) return;
        if (!layer) {
            await whepClient.unselectLayer();
            return;
        }
        // @ts-expect-error legacy WHEP client typing is incomplete here.
        await whepClient.selectLayer({ encodingId: layer }).catch((e) => {
            log(String(e));
        });
    };

    onCleanup(() => {
        clearDisconnectTimer();
        clearReconnectTimer();
        void stop({ reconnect: false });
    });

    return {
        stream,
        peerConnection,
        datachannel,
        status,
        error,
        audioTrackCount,
        videoTrackCount,
        play,
        stop,
        mute,
        selectLayer,
    };
}
