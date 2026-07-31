import { WHEPClient } from "@binbat/whip-whep/whep.js";

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

export type WhepPlaybackCoreState = {
    stream: MediaStream | null;
    peerConnection: RTCPeerConnection | null;
    datachannel: RTCDataChannel | null;
    status: WhepPlaybackStatus;
    error: Error | null;
    audioTrackCount: number;
    videoTrackCount: number;
};

export type WhepPlaybackCoreOptions = {
    url: () => string;
    token?: () => string;
    // When omitted or returning undefined, an ICE disconnect gets a 3s grace
    // period and never auto-reconnects. A value <= 0 stops immediately;
    // > 0 schedules a reconnect after that many milliseconds.
    reconnectMs?: () => number | undefined;
    createDataChannel?: boolean;
    log?: (message: string) => void;
};

export type WhepPlaybackCoreListener = (state: WhepPlaybackCoreState) => void;

function is404Error(error: unknown) {
    const maybe = error as { response?: { status?: number }; status?: number };
    const status = maybe?.response?.status ?? maybe?.status;
    return status === 404 || String(error).includes("404");
}

// Latency hints from the WebRTC Extensions spec (Chrome 125+). Both default
// to null, which selects the browser's adaptive jitter buffer. Setting them
// to 0 makes the receiver hand decoded frames to the renderer immediately;
// setting them back to null restores the default buffering behavior.
type RtpReceiverLatencyHints = RTCRtpReceiver & {
    playoutDelayHint?: number | null;
    jitterBufferTarget?: number | null;
};

function applyLatencyHints(receiver: RTCRtpReceiver, lowLatency: boolean) {
    const hinted = receiver as RtpReceiverLatencyHints;
    if ("jitterBufferTarget" in hinted) {
        hinted.jitterBufferTarget = lowLatency ? 0 : null;
    }
    if ("playoutDelayHint" in hinted) {
        hinted.playoutDelayHint = lowLatency ? 0 : null;
    }
}

// Framework-agnostic WHEP playback engine. Reactive-looking inputs are plain
// getters and every state change is pushed through `subscribe`, so any
// framework adapter (Solid signals, Vue refs, ...) can sit on top without
// the core knowing which one it is.
export class WhepPlaybackCore {
    private state: WhepPlaybackCoreState = {
        stream: null,
        peerConnection: null,
        datachannel: null,
        status: "idle",
        error: null,
        audioTrackCount: 0,
        videoTrackCount: 0,
    };

    private readonly options: WhepPlaybackCoreOptions;
    private listeners = new Set<WhepPlaybackCoreListener>();
    private whepClient: WHEPClient | null = null;
    private activePeerConnection: RTCPeerConnection | null = null;
    private lowLatency = false;
    private playToken = 0;
    private disconnectTimer: ReturnType<typeof setTimeout> | undefined;
    private reconnectTimer: ReturnType<typeof setTimeout> | undefined;

    constructor(options: WhepPlaybackCoreOptions) {
        this.options = options;
    }

    getState(): WhepPlaybackCoreState {
        return { ...this.state };
    }

    // The listener fires immediately with the current snapshot, then on every
    // state change. Returns an unsubscribe function.
    subscribe(listener: WhepPlaybackCoreListener): () => void {
        this.listeners.add(listener);
        listener(this.getState());
        return () => {
            this.listeners.delete(listener);
        };
    }

    async play(): Promise<void> {
        if (this.activePeerConnection) return;

        this.clearDisconnectTimer();
        this.clearReconnectTimer();
        this.setState({ status: "connecting", error: null });

        const pc = new RTCPeerConnection();
        const ms = new MediaStream();
        const client = new WHEPClient();
        const nextPlayToken = this.playToken + 1;
        this.playToken = nextPlayToken;

        this.whepClient = client;
        this.activePeerConnection = pc;
        this.setState({ peerConnection: pc, stream: ms });

        if (this.options.createDataChannel) {
            this.setState({ datachannel: pc.createDataChannel("") });
        }

        pc.addTransceiver("video", { direction: "recvonly" });
        pc.addTransceiver("audio", { direction: "recvonly" });

        const syncTrackCounts = () => {
            this.setState({
                audioTrackCount: ms.getAudioTracks().length,
                videoTrackCount: ms.getVideoTracks().length,
            });
        };

        pc.ontrack = (event) => {
            if (this.playToken !== nextPlayToken) return;
            this.log(`track: ${event.track.kind}`);
            applyLatencyHints(event.receiver, this.lowLatency);
            ms.addTrack(event.track);
            syncTrackCounts();
            this.setState({ stream: ms });
        };

        pc.oniceconnectionstatechange = () => {
            if (this.playToken !== nextPlayToken) return;
            this.log(`ICE State: ${pc.iceConnectionState}`);
            if (
                pc.iceConnectionState === "connected" ||
                pc.iceConnectionState === "completed"
            ) {
                this.clearDisconnectTimer();
                this.setState({ status: "playing" });
                return;
            }
            if (pc.iceConnectionState === "disconnected") {
                const reconnectMs = this.options.reconnectMs?.();
                const disconnectDelay =
                    reconnectMs === undefined ? 3000 : reconnectMs;
                const shouldReconnect =
                    reconnectMs !== undefined && reconnectMs > 0;

                this.clearDisconnectTimer();

                if (disconnectDelay <= 0) {
                    void this.stop({ reconnect: false });
                    return;
                }

                this.setState({ status: "reconnecting" });
                this.disconnectTimer = setTimeout(() => {
                    this.disconnectTimer = undefined;
                    void this.stop({ reconnect: shouldReconnect });
                }, disconnectDelay);
                return;
            }
            if (pc.iceConnectionState === "failed") {
                void this.stop({
                    reconnect: (this.options.reconnectMs?.() ?? 0) > 0,
                });
                return;
            }
            if (pc.iceConnectionState === "closed") {
                void this.stop({
                    reconnect: (this.options.reconnectMs?.() ?? 0) > 0,
                });
            }
        };

        pc.onconnectionstatechange = () => {
            if (this.playToken !== nextPlayToken) return;
            this.log(`connection State: ${pc.connectionState}`);
        };

        try {
            this.log("http begined");
            await client.view(
                pc,
                this.options.url(),
                this.options.token?.() ?? "",
            );
            if (this.playToken === nextPlayToken) {
                syncTrackCounts();
                this.setState({ status: "playing" });
            }
        } catch (playError) {
            if (this.playToken !== nextPlayToken) return;
            const err =
                playError instanceof Error
                    ? playError
                    : new Error(String(playError));
            this.setState({ error: err, status: "error" });
            this.log(`ERROR: ${String(playError)}`);
            await this.stop({
                reconnect: (this.options.reconnectMs?.() ?? 0) > 0,
            });
        }
    }

    async stop(options: { reconnect?: boolean } = {}): Promise<void> {
        const shouldReconnect = options.reconnect ?? false;
        const reconnectDelay = this.options.reconnectMs?.() ?? 0;
        const shouldScheduleReconnect = shouldReconnect && reconnectDelay > 0;
        const client = this.whepClient;
        const pc = this.activePeerConnection;
        this.whepClient = null;
        this.activePeerConnection = null;
        this.playToken += 1;
        this.clearDisconnectTimer();
        this.clearReconnectTimer();
        this.resetState();
        this.setState({
            status: shouldScheduleReconnect ? "reconnecting" : "stopped",
        });

        await this.cleanupClient(client);
        this.cleanupPeerConnection(pc);

        if (!shouldScheduleReconnect) {
            return;
        }

        this.reconnectTimer = setTimeout(() => {
            this.reconnectTimer = undefined;
            void this.play();
        }, reconnectDelay);
    }

    async mute(muted: WhepMute): Promise<void> {
        if (!this.whepClient) return;
        this.log(`mute: ${JSON.stringify(muted)}`);
        await this.whepClient.mute(muted);
    }

    async selectLayer(layer: string): Promise<void> {
        if (!this.whepClient) return;
        if (!layer) {
            await this.whepClient.unselectLayer();
            return;
        }
        // @ts-expect-error legacy WHEP client typing is incomplete here.
        await this.whepClient.selectLayer({ encodingId: layer }).catch((e) => {
            this.log(String(e));
        });
    }

    // Live-toggle the low-latency hints on every receiver of the active
    // peer connection. Receivers that arrive later pick the flag up in
    // `ontrack`.
    setLowLatency(enabled: boolean): void {
        this.lowLatency = enabled;
        const pc = this.activePeerConnection;
        if (!pc) return;
        for (const receiver of pc.getReceivers()) {
            applyLatencyHints(receiver, enabled);
        }
        this.log(`low latency (jitter buffer off): ${enabled}`);
    }

    async destroy(): Promise<void> {
        this.clearDisconnectTimer();
        this.clearReconnectTimer();
        await this.stop({ reconnect: false });
        this.listeners.clear();
    }

    private log(message: string): void {
        this.options.log?.(message);
    }

    private setState(patch: Partial<WhepPlaybackCoreState>): void {
        Object.assign(this.state, patch);
        const snapshot = this.getState();
        for (const listener of this.listeners) {
            listener(snapshot);
        }
    }

    private clearReconnectTimer(): void {
        if (this.reconnectTimer !== undefined) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = undefined;
        }
    }

    private clearDisconnectTimer(): void {
        if (this.disconnectTimer !== undefined) {
            clearTimeout(this.disconnectTimer);
            this.disconnectTimer = undefined;
        }
    }

    private resetState(): void {
        this.setState({
            stream: null,
            peerConnection: null,
            datachannel: null,
            audioTrackCount: 0,
            videoTrackCount: 0,
        });
    }

    private cleanupPeerConnection(pc: RTCPeerConnection | null): void {
        if (!pc) return;
        pc.ontrack = null;
        pc.oniceconnectionstatechange = null;
        pc.onconnectionstatechange = null;
        try {
            pc.close();
        } catch {
            // Ignore close errors during teardown.
        }
    }

    private async cleanupClient(client: WHEPClient | null): Promise<void> {
        if (!client) return;
        try {
            await client.stop();
        } catch (stopError) {
            if (!is404Error(stopError)) {
                this.log(`stop error: ${String(stopError)}`);
            }
        }
    }
}
