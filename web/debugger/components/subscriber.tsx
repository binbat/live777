import { useSearchParams } from "@solidjs/router";
import { createWhepPlayback } from "player-core";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import {
    collectVideoRtpFps,
    type VideoFpsSamples,
} from "../../player-core/webrtc-stats";
import {
    DefaultQRCodeFrameRate,
    parseQRCodeFrameRate,
    type QRCodeFrameRate,
    QRCodeStreamDecoder,
} from "../../shared/qrcode-stream";
import { createLogger } from "../primitive/logger";
import Datachannel from "./datachannel";
import Player from "./player";

const WhepLayerOptions = [
    { value: "", text: "AUTO" },
    { value: "q", text: "LOW" },
    { value: "h", text: "MEDIUM" },
    { value: "f", text: "HIGH" },
];

function formatFps(fps: number | null) {
    return fps === null ? "--" : fps.toFixed(1);
}

export default function Subscriber() {
    const [searchParams] = useSearchParams();

    const [disabled, setDisabled] = createSignal(true);
    const [disabledAudio, setDisabledAudio] = createSignal(false);
    const [disabledVideo, setDisabledVideo] = createSignal(false);
    const [logs, setLogs, clear] = createLogger();

    const [latency, setLatency] = createSignal("");
    const [isMeasuringQrLatency, setIsMeasuringQrLatency] = createSignal(false);
    const [expectedQrFrameRate, setExpectedQrFrameRate] =
        createSignal<QRCodeFrameRate>(
            parseQRCodeFrameRate(searchParams.qrfps ?? DefaultQRCodeFrameRate),
        );
    const [actualReceiveFps, setActualReceiveFps] = createSignal<number | null>(
        null,
    );

    let videoRef: HTMLVideoElement | undefined;
    let decoder: QRCodeStreamDecoder | null = null;
    let receiveFpsSamples: VideoFpsSamples = {};
    let receiveFpsInterval: ReturnType<typeof setInterval> | null = null;
    let receiveFpsToken = 0;

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
        stopActualReceiveFps();
        stopQrLatencyMeasure();
        void playback.stop({ reconnect: false });
    });

    createEffect(() => {
        if (!playback.stream()) {
            stopQrLatencyMeasure();
        }
    });

    createEffect(() => {
        const peerConnection = playback.peerConnection();
        if (peerConnection) {
            startActualReceiveFps(peerConnection);
        } else {
            stopActualReceiveFps();
        }
    });

    function stopActualReceiveFps() {
        receiveFpsToken += 1;
        if (receiveFpsInterval) {
            clearInterval(receiveFpsInterval);
            receiveFpsInterval = null;
        }
        receiveFpsSamples = {};
        setActualReceiveFps(null);
    }

    function startActualReceiveFps(peerConnection: RTCPeerConnection) {
        stopActualReceiveFps();
        const token = receiveFpsToken;

        const syncActualReceiveFps = async () => {
            const stats = await collectVideoRtpFps(
                peerConnection,
                "inbound",
                receiveFpsSamples,
            );
            if (token !== receiveFpsToken) {
                return;
            }
            receiveFpsSamples = stats.samples;
            setActualReceiveFps(stats.fps);
        };

        void syncActualReceiveFps();
        receiveFpsInterval = setInterval(() => {
            void syncActualReceiveFps();
        }, 1000);
    }

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
        setIsMeasuringQrLatency(false);
        setLatency("");
    }

    function startQrLatencyMeasure() {
        if (!videoRef || !playback.stream()) {
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

                <section>Expected QR FPS: {expectedQrFrameRate()} fps</section>

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
                        {` | Expected FPS: ${expectedQrFrameRate()} | Actual Receive FPS: ${formatFps(actualReceiveFps())}`}
                        {latency() && ` | Latency: ${latency()}`}
                    </h5>
                    <Show when={playback.stream()}>
                        {(s) => {
                            const stream = s();
                            return (
                                <Player
                                    stream={stream}
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
