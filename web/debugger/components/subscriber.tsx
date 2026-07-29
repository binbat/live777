import { useSearchParams } from "@solidjs/router";
import { createWhepPlayback } from "player-core";
import {
    createEffect,
    createMemo,
    createSignal,
    onCleanup,
    Show,
} from "solid-js";
import {
    DefaultQRCodeFrameRate,
    parseQRCodeFrameRate,
    type QRCodeFrameRate,
    QRCodeStreamDecoder,
} from "../../shared/qrcode-stream";
import { createLogger } from "../primitive/logger";
import Datachannel from "./datachannel";
import Player from "./player";
import {
    getQrLatencyDisplay,
    type QrLatencySample,
} from "./qr-latency-freshness";

const WhepLayerOptions = [
    { value: "", text: "AUTO" },
    { value: "q", text: "LOW" },
    { value: "h", text: "MEDIUM" },
    { value: "f", text: "HIGH" },
];

export default function Subscriber() {
    const [searchParams] = useSearchParams();

    const [disabled, setDisabled] = createSignal(true);
    const [disabledAudio, setDisabledAudio] = createSignal(false);
    const [disabledVideo, setDisabledVideo] = createSignal(false);
    const [logs, setLogs, clear] = createLogger();

    const [isMeasuringQrLatency, setIsMeasuringQrLatency] = createSignal(false);
    const [measurementStartedAt, setMeasurementStartedAt] = createSignal<
        number | null
    >(null);
    const [latestLatencySample, setLatestLatencySample] =
        createSignal<QrLatencySample | null>(null);
    const [freshnessNow, setFreshnessNow] = createSignal(0);
    const [expectedQrFrameRate, setExpectedQrFrameRate] =
        createSignal<QRCodeFrameRate>(
            parseQRCodeFrameRate(searchParams.qrfps ?? DefaultQRCodeFrameRate),
        );

    let videoRef: HTMLVideoElement | undefined;
    let decoder: QRCodeStreamDecoder | null = null;
    let freshnessTimer: number | null = null;

    const latencyDisplay = createMemo(() => {
        const startedAt = measurementStartedAt();
        if (startedAt === null) {
            return null;
        }
        return getQrLatencyDisplay(
            freshnessNow(),
            startedAt,
            latestLatencySample(),
        );
    });

    const playback = createWhepPlayback({
        url: () => {
            const streamId = ((searchParams.id as string) || "").trim();
            return `${location.origin}/whep/${encodeURIComponent(streamId)}`;
        },
        token: () => (searchParams.token as string) || "",
        createDataChannel: true,
        log: setLogs,
    });

    onCleanup(() => {
        stopQrLatencyMeasure();
        void playback.stop({ reconnect: false });
    });

    createEffect(() => {
        if (!playback.stream()) {
            stopQrLatencyMeasure();
        }
    });

    createEffect(() => {
        const frameRate = parseQRCodeFrameRate(
            searchParams.qrfps ?? DefaultQRCodeFrameRate,
        );
        if (frameRate !== expectedQrFrameRate()) {
            setExpectedQrFrameRate(frameRate);
        }
    });

    function stopQrLatencyMeasure() {
        if (decoder) {
            decoder.stop();
            decoder = null;
        }
        if (freshnessTimer !== null) {
            window.clearInterval(freshnessTimer);
            freshnessTimer = null;
        }
        setIsMeasuringQrLatency(false);
        setMeasurementStartedAt(null);
        setLatestLatencySample(null);
        setFreshnessNow(0);
    }

    function startQrLatencyMeasure() {
        if (!videoRef || !playback.stream()) {
            return;
        }
        stopQrLatencyMeasure();
        const startedAt = performance.now();
        setIsMeasuringQrLatency(true);
        setMeasurementStartedAt(startedAt);
        setFreshnessNow(startedAt);
        freshnessTimer = window.setInterval(() => {
            setFreshnessNow(performance.now());
        }, 250);
        decoder = new QRCodeStreamDecoder(videoRef);
        decoder.addEventListener("latency", (e: CustomEvent<number>) => {
            const receivedAt = performance.now();
            setLatestLatencySample({
                latencyMs: e.detail,
                receivedAtMs: receivedAt,
            });
            setFreshnessNow(receivedAt);
        });
        decoder.start();
    }

    const start = async () => {
        clear();
        stopQrLatencyMeasure();
        const streamId = ((searchParams.id as string) || "").trim();
        if (!streamId) {
            setLogs("Stream ID is required before subscribing.");
            return;
        }
        await playback.play();
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
                        onChange={(e) => playback.selectLayer(e.target.value)}
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
                            void playback.mute({
                                kind: "audio",
                                enabled: disabled,
                            });
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
                            void playback.mute({
                                kind: "video",
                                enabled: disabled,
                            });
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
                            void playback.stop({ reconnect: false });
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
                            disabled() ||
                            !playback.stream() ||
                            isMeasuringQrLatency()
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
                        Audio Track Count: {playback.audioTrackCount()}, Video
                        Track Count: {playback.videoTrackCount()}
                    </h5>
                    <h5>
                        QR Target FPS: {expectedQrFrameRate()}
                        <Show when={latencyDisplay()}>
                            {(display) => (
                                <span
                                    class={`qr-latency qr-latency-${display().state}`}
                                >
                                    {" | Latency: "}
                                    {display().value} | {display().status}
                                </span>
                            )}
                        </Show>
                    </h5>
                    <Show when={playback.stream()}>
                        {(s) => {
                            const stream = s();
                            return (
                                <Player
                                    stream={stream}
                                    showRenderFps
                                    onVideoElement={(video) => {
                                        videoRef = video;
                                    }}
                                    getPeerConnection={() =>
                                        playback.peerConnection()
                                    }
                                />
                            );
                        }}
                    </Show>
                </section>
                <section>
                    <Show when={playback.datachannel()}>
                        {(dc) => {
                            const datachannel = dc();
                            return <Datachannel datachannel={datachannel} />;
                        }}
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
