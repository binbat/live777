import { useSearchParams } from "@solidjs/router";
import { createSignal, onCleanup, Show } from "solid-js";
import { QRCodeStream } from "../../shared/qrcode-stream";
import { createLogger } from "../primitive/logger";
import Datachannel from "./datachannel";
import Device from "./device";
import Player from "./player";
import publish from "./publish";

const AudioCodecOptions = [
    { value: "", text: "default" },
    { value: "opus", text: "OPUS" },
    { value: "g722", text: "G722" },
];

const VideoCodecOptions = [
    { value: "", text: "default" },
    { value: "av1", text: "AV1" },
    { value: "vp9", text: "VP9" },
    { value: "vp8", text: "VP8" },
    { value: "h264", text: "H264" },
    { value: "h265", text: "H265" },
];

const mapCodec: Record<string, string> = {
    "": "",
    av1: "av1/90000",
    vp9: "vp9/90000",
    vp8: "vp8/90000",
    h264: "h264/90000",
    h265: "h265/90000",
    opus: "opus/48000",
    g722: "g722/8000",
};

const VideoWidthOptions = [
    { value: "", text: "default" },
    { value: "320", text: "320px" },
    { value: "480", text: "480px" },
    { value: "600", text: "600px" },
    { value: "1280", text: "1280px" },
    { value: "1920", text: "1920px" },
    { value: "3480", text: "3480px" },
];

const VideoHeightOptions = [
    { value: "", text: "default" },
    { value: "240", text: "240px" },
    { value: "320", text: "320px" },
    { value: "480", text: "480px" },
    { value: "720", text: "720px" },
    { value: "1080", text: "1080px" },
    { value: "2160", text: "2160px" },
];

const WhipLayerOptions = [
    { value: "f", text: "Base" },
    { value: "h", text: "Base + 1/2" },
    { value: "q", text: "Base + 1/2 + 1/4" },
];

type SourceMode = "device" | "desktop" | "qrtime";
type QrState = "idle" | "previewing" | "publishing";

const QrCanvasWidth = 480;
const QrCanvasHeight = 320;

export default function Publisher() {
    const [disabled, setDisabled] = createSignal(false);
    const [stream, setStream] = createSignal<MediaStream | null>(null);
    const [preparedDesktopStream, setPreparedDesktopStream] =
        createSignal<MediaStream | null>(null);
    const [datachannel, setDatachannel] = createSignal<RTCDataChannel | null>(
        null,
    );
    const [sourceMode, setSourceMode] = createSignal<SourceMode>("device");
    const [qrState, setQrState] = createSignal<QrState>("idle");
    const [deviceRefreshToken, setDeviceRefreshToken] = createSignal(0);
    const [selectAudioDevice, setSelectAudioDevice] = createSignal("");
    const [selectVideoDevice, setSelectVideoDevice] = createSignal("");
    const [selectVideoWidth, setSelectVideoWidth] = createSignal("");
    const [selectVideoHeight, setSelectVideoHeight] = createSignal("");
    const [selectAudioPseudo, setSelectAudioPseudo] = createSignal(false);
    const [selectVideoLayer, setSelectVideoLayer] = createSignal("f");

    const [audioTrackCount, setAudioTrackCount] = createSignal(0);
    const [videoTrackCount, setVideoTrackCount] = createSignal(0);

    const [searchParams, setSearchParams] = useSearchParams();

    const [logs, setLogs, clear] = createLogger();

    let stop: (() => Promise<void>) | undefined;
    let qrCanvasRef: HTMLCanvasElement | undefined;
    let qrStream: QRCodeStream | null = null;
    let desktopStreamCleanupInProgress = false;

    const updatePreviewStream = (currentStream: MediaStream | null) => {
        setAudioTrackCount(
            currentStream ? currentStream.getAudioTracks().length : 0,
        );
        setVideoTrackCount(
            currentStream ? currentStream.getVideoTracks().length : 0,
        );
        setStream(currentStream);
    };

    const clearPreparedDesktopStream = ({
        stopTracks = true,
        clearPreview = true,
    }: {
        stopTracks?: boolean;
        clearPreview?: boolean;
    } = {}) => {
        const currentStream = preparedDesktopStream();
        if (!currentStream) {
            if (clearPreview) {
                updatePreviewStream(null);
            }
            return;
        }
        desktopStreamCleanupInProgress = true;
        currentStream.getTracks().forEach((track) => {
            track.onended = null;
            if (stopTracks && track.readyState === "live") {
                track.stop();
            }
        });
        desktopStreamCleanupInProgress = false;
        setPreparedDesktopStream(null);
        if (clearPreview) {
            updatePreviewStream(null);
        }
    };

    const clearQrStream = ({
        clearPreview = true,
    }: {
        clearPreview?: boolean;
    } = {}) => {
        if (qrStream) {
            qrStream.stop();
            qrStream = null;
        }
        setQrState("idle");
        if (clearPreview) {
            updatePreviewStream(null);
        }
    };

    onCleanup(async () => {
        if (stop) {
            await stop();
            stop = undefined;
        }
        clearQrStream();
        clearPreparedDesktopStream();
    });

    const ensureQrInputStream = () => {
        if (!qrCanvasRef) {
            return null;
        }
        qrCanvasRef.width = QrCanvasWidth;
        qrCanvasRef.height = QrCanvasHeight;
        if (!qrStream) {
            qrStream = new QRCodeStream(qrCanvasRef);
        }
        return qrStream.capture();
    };

    const prepareQrStream = () => {
        clear();
        clearPreparedDesktopStream();
        clearQrStream();
        setSourceMode("qrtime");

        const inputStream = ensureQrInputStream();
        if (!inputStream) {
            setLogs("QRCode Time stream initialization failed.");
            return;
        }

        updatePreviewStream(inputStream);
        setQrState("previewing");
        setLogs("QR source ready. Click Start to publish.");
    };

    const handleDesktopStreamEnded = async () => {
        if (desktopStreamCleanupInProgress) {
            return;
        }
        clear();
        setLogs("Desktop sharing ended.");
        if (disabled()) {
            await stopPublishing();
            return;
        }
        clearPreparedDesktopStream();
    };

    const prepareDesktopStream = async () => {
        clear();
        setSourceMode("desktop");
        clearQrStream();
        clearPreparedDesktopStream();

        const videoWidth = parseInt(selectVideoWidth(), 10) || undefined;
        const videoHeight = parseInt(selectVideoHeight(), 10) || undefined;
        const videoConstraints: MediaTrackConstraints = {};
        if (videoWidth) {
            videoConstraints.width = videoWidth;
        }
        if (videoHeight) {
            videoConstraints.height = videoHeight;
        }

        try {
            const currentStream = await navigator.mediaDevices.getDisplayMedia({
                audio: true,
                video: videoConstraints,
            });
            currentStream.getTracks().forEach((track) => {
                track.onended = () => {
                    void handleDesktopStreamEnded();
                };
            });
            setPreparedDesktopStream(currentStream);
            updatePreviewStream(currentStream);
            setLogs("Desktop source ready. Click Start to publish.");
        } catch (e) {
            const error =
                e instanceof Error
                    ? `${e.name}: ${e.message}`
                    : "unknown error";
            setLogs(`Desktop sharing was not started. ${error}`);
        }
    };

    const start = async () => {
        setDisabled(true);
        clear();

        const isDesktopMode = sourceMode() === "desktop";
        const isQrMode = sourceMode() === "qrtime";
        const inputStream = isDesktopMode
            ? preparedDesktopStream()
            : isQrMode
              ? ensureQrInputStream()
              : null;
        if (isDesktopMode && !inputStream) {
            setLogs(
                "Click Share Desktop to choose a screen before publishing.",
            );
            setDisabled(false);
            return;
        }
        if (isQrMode && !inputStream) {
            setLogs("QRCode Time stream initialization failed.");
            setDisabled(false);
            return;
        }
        if (isQrMode && qrState() !== "previewing") {
            setLogs("Click QRCode Time to generate a QR preview first.");
            setDisabled(false);
            return;
        }

        stop = await publish({
            url: `${location.origin}/whip/${searchParams.id || "-"}`,
            token: (searchParams.token as string) || "",
            sourceMode: sourceMode(),
            inputStream,
            audio: {
                device: isDesktopMode || isQrMode ? "" : selectAudioDevice(),
                codec: mapCodec[(searchParams.acodec as string) || ""],
                pseudo: sourceMode() === "device" && selectAudioPseudo(),
            },
            video: {
                device: isDesktopMode || isQrMode ? "" : selectVideoDevice(),
                codec: mapCodec[(searchParams.vcodec as string) || ""],
                layer: selectVideoLayer(),
                width: parseInt(selectVideoWidth(), 10) || null,
                height: parseInt(selectVideoHeight(), 10) || null,
            },
            onStream: (currentStream: MediaStream | null): void => {
                updatePreviewStream(currentStream);
            },
            onChannel: (channel: RTCDataChannel): void => {
                setDatachannel(channel);
            },
            log: setLogs,
        });
        if (isQrMode) {
            setQrState("publishing");
        }
    };

    const stopPublishing = async () => {
        setDisabled(false);
        if (sourceMode() === "desktop") {
            clearPreparedDesktopStream({
                stopTracks: false,
                clearPreview: false,
            });
        }
        if (stop) {
            await stop();
            stop = undefined;
        }
        if (sourceMode() === "qrtime") {
            clearQrStream();
        }
        if (sourceMode() !== "desktop") {
            updatePreviewStream(null);
        }
    };

    const useDeviceSource = () => {
        clearPreparedDesktopStream();
        clearQrStream();
        setSourceMode("device");
        setDeviceRefreshToken((token) => token + 1);
    };

    const useQrTimeSource = () => {
        prepareQrStream();
    };

    return (
        <>
            <legend>WHIP</legend>
            <div style="text-align: center;">
                <canvas ref={qrCanvasRef} style="display: none;" />

                <section style="margin-bottom: 0.6rem; display: flex; justify-content: center; gap: 0.5rem; flex-wrap: wrap;">
                    <button
                        type="button"
                        disabled={disabled()}
                        onClick={useDeviceSource}
                    >
                        Use Device
                    </button>
                    <button
                        type="button"
                        disabled={disabled()}
                        onClick={() => {
                            void prepareDesktopStream();
                        }}
                    >
                        Share Desktop
                    </button>
                    <button
                        type="button"
                        disabled={disabled()}
                        onClick={useQrTimeSource}
                    >
                        QRCode Time
                    </button>
                </section>

                <section>
                    <span>Mode: {sourceMode()}</span>
                </section>

                <Show when={sourceMode() === "device"}>
                    <section>
                        <Device
                            disabled={disabled()}
                            refreshToken={deviceRefreshToken()}
                            onSelectAudio={(deviceId) =>
                                setSelectAudioDevice(deviceId)
                            }
                            onSelectVideo={(deviceId) =>
                                setSelectVideoDevice(deviceId)
                            }
                        />
                    </section>
                </Show>

                <section>
                    <label>
                        Audio Codec:
                        <select
                            onChange={(e) => {
                                setSearchParams({ acodec: e.target.value });
                            }}
                            disabled={disabled()}
                            value={(searchParams.acodec as string) || ""}
                        >
                            {AudioCodecOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        Video Codec:
                        <select
                            onChange={(e) => {
                                setSearchParams({ vcodec: e.target.value });
                            }}
                            disabled={disabled()}
                            value={(searchParams.vcodec as string) || ""}
                        >
                            {VideoCodecOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                </section>
                <section>
                    <label>
                        Video Width:
                        <select
                            onChange={(e) => {
                                setSelectVideoWidth(e.target.value);
                            }}
                            disabled={disabled()}
                        >
                            {VideoWidthOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        Video Height:
                        <select
                            disabled={disabled()}
                            onChange={(e) => {
                                setSelectVideoHeight(e.target.value);
                            }}
                        >
                            {VideoHeightOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                </section>
                <section>
                    <input
                        type="checkbox"
                        checked={selectAudioPseudo()}
                        onChange={(e) => {
                            setSelectAudioPseudo(e.target.checked);
                        }}
                        disabled={disabled() || sourceMode() !== "device"}
                    />
                    Use Pseudo Audio Track
                </section>
                <section>
                    <label>
                        SVC Layer:
                        <select
                            onChange={(e) => {
                                setSelectVideoLayer(e.target.value);
                            }}
                            disabled={disabled()}
                        >
                            {WhipLayerOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                </section>
                <section>
                    <button
                        type="button"
                        onClick={start}
                        disabled={
                            disabled() ||
                            (sourceMode() === "qrtime" &&
                                qrState() !== "previewing") ||
                            (sourceMode() === "desktop" &&
                                !preparedDesktopStream())
                        }
                    >
                        Start
                    </button>
                    <button
                        type="button"
                        onClick={stopPublishing}
                        disabled={!disabled()}
                    >
                        Stop
                    </button>
                </section>
                <Show when={sourceMode() === "desktop" && !disabled()}>
                    <section>
                        <small>
                            {preparedDesktopStream()
                                ? "Desktop source ready. Click Start to publish."
                                : "Click Share Desktop to choose a screen, window, or tab."}
                        </small>
                    </section>
                </Show>
                <Show when={sourceMode() === "qrtime" && !disabled()}>
                    <section>
                        <small>
                            {qrState() === "previewing"
                                ? "QR source ready. Click Start to publish."
                                : qrState() === "publishing"
                                  ? "QR source is publishing."
                                  : "Click QRCode Time to generate a QR preview."}
                        </small>
                    </section>
                    <section>
                        <span>QR State: {qrState()}</span>
                    </section>
                </Show>
                <section>
                    <h3>WHIP Video:</h3>
                    <h5>
                        Audio Track Count: {audioTrackCount()}, Video Track
                        Count: {videoTrackCount()}
                    </h5>
                    <Show when={stream()}>
                        {(ms) => <Player stream={ms()} />}
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
