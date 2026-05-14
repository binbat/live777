import { useSearchParams } from "@solidjs/router";
import { createSignal, onCleanup, Show } from "solid-js";
import { QRCodeStreamDecoder } from "../../shared/qrcode-stream";
import { createLogger } from "../primitive/logger";
import Datachannel from "./datachannel";
import Player from "./player";

import subscribe from "./subscribe";

const WhepLayerOptions = [
    { value: "", text: "AUTO" },
    { value: "q", text: "LOW" },
    { value: "h", text: "MEDIUM" },
    { value: "f", text: "HIGH" },
];

export default function Subscriber() {
    const [disabled, setDisabled] = createSignal(true);
    const [disabledAudio, setDisabledAudio] = createSignal(false);
    const [disabledVideo, setDisabledVideo] = createSignal(false);
    const [stream, setStream] = createSignal<MediaStream | null>(null);
    const [peerConnection, setPeerConnection] =
        createSignal<RTCPeerConnection | null>(null);
    const [datachannel, setDatachannel] = createSignal<RTCDataChannel | null>(
        null,
    );
    const [logs, setLogs, clear] = createLogger();

    const [audioTrackCount, setAudioTrackCount] = createSignal(0);
    const [videoTrackCount, setVideoTrackCount] = createSignal(0);
    const [latency, setLatency] = createSignal("");
    const [isMeasuringQrLatency, setIsMeasuringQrLatency] = createSignal(false);

    const [searchParams] = useSearchParams();
    let videoRef: HTMLVideoElement | undefined;
    let decoder: QRCodeStreamDecoder | null = null;
    let stop: () => Promise<void> | undefined;
    // biome-ignore lint/suspicious/noExplicitAny: This whip-whep.js use any type
    let mute: (muted: any) => Promise<void> | undefined;
    let selectLayer: (layer: string) => Promise<void> | undefined;

    onCleanup(() => {
        stopQrLatencyMeasure();
    });

    const stopQrLatencyMeasure = () => {
        if (decoder) {
            decoder.stop();
            decoder = null;
        }
        setIsMeasuringQrLatency(false);
        setLatency("");
    };

    const startQrLatencyMeasure = () => {
        if (!videoRef || !stream()) {
            return;
        }
        stopQrLatencyMeasure();
        setIsMeasuringQrLatency(true);
        setLatency("-- ms");
        decoder = new QRCodeStreamDecoder(videoRef);
        decoder.addEventListener("latency", (e: CustomEvent<number>) => {
            setLatency(`${e.detail} ms`);
        });
        decoder.start();
    };

    const start = async () => {
        clear();
        stopQrLatencyMeasure();
        const streamId = ((searchParams.id as string) || "").trim();
        if (!streamId) {
            setLogs("Stream ID is required before subscribing.");
            return;
        }
        [stop, mute, selectLayer] = await subscribe({
            url: `${location.origin}/whep/${encodeURIComponent(streamId)}`,
            token: (searchParams.token as string) || "",
            onStream: (stream: MediaStream | null): void => {
                setAudioTrackCount(stream ? stream.getAudioTracks().length : 0);
                setVideoTrackCount(stream ? stream.getVideoTracks().length : 0);
                setStream(stream);
                if (!stream) {
                    stopQrLatencyMeasure();
                }
            },
            onChannel: (channel: RTCDataChannel): void => {
                setDatachannel(channel);
            },
            onPeerConnection: (pc: RTCPeerConnection | null): void => {
                setPeerConnection(pc);
            },
            log: setLogs,
        });
        setDisabled(false);
    };

    return (
        <>
            <legend>WHEP</legend>
            <div style="text-align: center;">
                <section>
                    SVC Layer:{" "}
                    <select
                        disabled={disabled()}
                        onChange={(e) => selectLayer(e.target.value)}
                    >
                        {WhepLayerOptions.map((o) => (
                            <option value={o.value}>{o.text}</option>
                        ))}
                    </select>
                </section>
                <section>
                    <button
                        type="button"
                        disabled={disabled()}
                        onClick={() => {
                            const disabled = disabledAudio();
                            setDisabledAudio(!disabled);
                            mute({ kind: "audio", enabled: disabled });
                        }}
                    >
                        {disabledAudio() ? "Enable" : "Disable"} Audio
                    </button>
                    <button
                        type="button"
                        disabled={disabled()}
                        onClick={() => {
                            const disabled = disabledVideo();
                            setDisabledVideo(!disabled);
                            mute({ kind: "video", enabled: disabled });
                        }}
                    >
                        {disabledVideo() ? "Enable" : "Disable"} Video
                    </button>
                </section>
                <section>
                    <button
                        type="button"
                        onClick={start}
                        disabled={!disabled()}
                    >
                        Start
                    </button>
                    <button
                        type="button"
                        onClick={() => {
                            stopQrLatencyMeasure();
                            stop();
                            setDisabled(true);
                        }}
                        disabled={disabled()}
                    >
                        Stop
                    </button>
                </section>

                <section>
                    <button
                        type="button"
                        onClick={startQrLatencyMeasure}
                        disabled={
                            disabled() || !stream() || isMeasuringQrLatency()
                        }
                    >
                        Measure QR Latency
                    </button>
                    <button
                        type="button"
                        onClick={stopQrLatencyMeasure}
                        disabled={!isMeasuringQrLatency()}
                    >
                        Stop Measuring
                    </button>
                </section>

                <section>
                    <h3>WHEP Video:</h3>
                    <h5>
                        Audio Track Count: {audioTrackCount()}, Video Track
                        Count: {videoTrackCount()}
                        {latency() && ` | Latency: ${latency()}`}
                    </h5>
                    <Show when={stream()}>
                        {(s) => (
                            <Player
                                stream={s()}
                                onVideoElement={(video) => {
                                    videoRef = video;
                                }}
                                getPeerConnection={() => peerConnection()}
                            />
                        )}
                    </Show>
                </section>
                <section>
                    <Show when={datachannel()}>
                        {(dc) => <Datachannel datachannel={dc()} />}
                    </Show>
                </section>
                <section>
                    <h4>Logs:</h4>
                    <pre>{logs().join("\n")}</pre>
                </section>
            </div>
        </>
    );
}
