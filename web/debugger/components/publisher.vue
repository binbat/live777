<script setup lang="ts">
import {
    computed,
    onUnmounted,
    ref,
    shallowRef,
    useTemplateRef,
    watchEffect,
} from "vue";
import {
    parseQRCodeFrameRate,
    type QRCodeFrameRate,
    QRCodeFrameRates,
    QRCodeStream,
} from "../../shared/qrcode-stream";
import { useLogger } from "../primitive/logger";
import { useSearchParams } from "../primitive/search-params";
import Datachannel from "./datachannel.vue";
import Device from "./device.vue";
import Player from "./player.vue";
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

const [searchParams, setSearchParams] = useSearchParams();

const disabled = ref(false);
const stream = shallowRef<MediaStream | null>(null);
const preparedDesktopStream = shallowRef<MediaStream | null>(null);
const datachannel = shallowRef<RTCDataChannel | null>(null);
const sourceMode = ref<SourceMode>("device");
const qrState = ref<QrState>("idle");
const deviceRefreshToken = ref(0);
const selectAudioDevice = ref("");
const selectVideoDevice = ref("");
const selectVideoWidth = ref("");
const selectVideoHeight = ref("");
const selectAudioPseudo = ref(false);
const selectVideoLayer = ref("f");
const selectQrFrameRate = ref<QRCodeFrameRate>(
    parseQRCodeFrameRate(searchParams.qrfps),
);

const audioTrackCount = ref(0);
const videoTrackCount = ref(0);

const [logs, setLogs, clear] = useLogger();

let stop: (() => Promise<void>) | undefined;
const qrCanvasRef = useTemplateRef<HTMLCanvasElement>("qrCanvasRef");
let qrStream: QRCodeStream | null = null;
let desktopStreamCleanupInProgress = false;

const selectedVideoCodec = computed(() => searchParams.vcodec || "");

const updatePreviewStream = (currentStream: MediaStream | null) => {
    audioTrackCount.value = currentStream
        ? currentStream.getAudioTracks().length
        : 0;
    videoTrackCount.value = currentStream
        ? currentStream.getVideoTracks().length
        : 0;
    stream.value = currentStream;
};

const clearPreparedDesktopStream = ({
    stopTracks = true,
    clearPreview = true,
}: {
    stopTracks?: boolean;
    clearPreview?: boolean;
} = {}) => {
    const currentStream = preparedDesktopStream.value;
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
    preparedDesktopStream.value = null;
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
    qrState.value = "idle";
    if (clearPreview) {
        updatePreviewStream(null);
    }
};

onUnmounted(async () => {
    if (stop) {
        await stop();
        stop = undefined;
    }
    clearQrStream();
    clearPreparedDesktopStream();
});

const ensureQrInputStream = () => {
    const canvas = qrCanvasRef.value;
    if (!canvas) {
        return null;
    }
    canvas.width = QrCanvasWidth;
    canvas.height = QrCanvasHeight;
    if (!qrStream) {
        qrStream = new QRCodeStream(canvas, selectQrFrameRate.value);
    }
    return qrStream.capture();
};

const prepareQrStream = () => {
    clear();
    clearPreparedDesktopStream();
    clearQrStream();
    sourceMode.value = "qrtime";

    const inputStream = ensureQrInputStream();
    if (!inputStream) {
        setLogs("QRCode Time stream initialization failed.");
        return;
    }

    updatePreviewStream(inputStream);
    qrState.value = "previewing";
    setLogs("QR source ready. Click Start to publish.");
};

const handleDesktopStreamEnded = async () => {
    if (desktopStreamCleanupInProgress) {
        return;
    }
    clear();
    setLogs("Desktop sharing ended.");
    if (disabled.value) {
        await stopPublishing();
        return;
    }
    clearPreparedDesktopStream();
};

const prepareDesktopStream = async () => {
    clear();
    sourceMode.value = "desktop";
    clearQrStream();
    clearPreparedDesktopStream();

    const videoWidth = parseInt(selectVideoWidth.value, 10) || undefined;
    const videoHeight = parseInt(selectVideoHeight.value, 10) || undefined;
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
        preparedDesktopStream.value = currentStream;
        updatePreviewStream(currentStream);
        setLogs("Desktop source ready. Click Start to publish.");
    } catch (e) {
        const error =
            e instanceof Error ? `${e.name}: ${e.message}` : "unknown error";
        setLogs(`Desktop sharing was not started. ${error}`);
    }
};

const start = async () => {
    disabled.value = true;
    clear();
    const streamId = (searchParams.id || "").trim();
    if (!streamId) {
        setLogs("Stream ID is required before publishing.");
        disabled.value = false;
        return;
    }

    const isDesktopMode = sourceMode.value === "desktop";
    const isQrMode = sourceMode.value === "qrtime";
    const inputStream = isDesktopMode
        ? preparedDesktopStream.value
        : isQrMode
          ? ensureQrInputStream()
          : null;
    if (isDesktopMode && !inputStream) {
        setLogs("Click Share Desktop to choose a screen before publishing.");
        disabled.value = false;
        return;
    }
    if (isQrMode && !inputStream) {
        setLogs("QRCode Time stream initialization failed.");
        disabled.value = false;
        return;
    }
    if (isQrMode && qrState.value !== "previewing") {
        setLogs("Click QRCode Time to generate a QR preview first.");
        disabled.value = false;
        return;
    }

    stop = await publish({
        url: `${location.origin}/whip/${encodeURIComponent(streamId)}`,
        token: searchParams.token || "",
        sourceMode: sourceMode.value,
        inputStream,
        audio: {
            device: isDesktopMode || isQrMode ? "" : selectAudioDevice.value,
            codec: mapCodec[searchParams.acodec || ""],
            pseudo: sourceMode.value === "device" && selectAudioPseudo.value,
        },
        video: {
            device: isDesktopMode || isQrMode ? "" : selectVideoDevice.value,
            codec: mapCodec[selectedVideoCodec.value],
            layer: selectVideoLayer.value,
            width: parseInt(selectVideoWidth.value, 10) || null,
            height: parseInt(selectVideoHeight.value, 10) || null,
        },
        onStream: (currentStream: MediaStream | null): void => {
            updatePreviewStream(currentStream);
        },
        onChannel: (channel: RTCDataChannel): void => {
            datachannel.value = channel;
        },
        log: setLogs,
    });
    if (isQrMode) {
        qrState.value = "publishing";
    }
};

const stopPublishing = async () => {
    disabled.value = false;
    if (sourceMode.value === "desktop") {
        clearPreparedDesktopStream({
            stopTracks: false,
            clearPreview: false,
        });
    }
    if (stop) {
        await stop();
        stop = undefined;
    }
    if (sourceMode.value === "qrtime") {
        clearQrStream();
    }
    if (sourceMode.value !== "desktop") {
        updatePreviewStream(null);
    }
};

const useDeviceSource = () => {
    clearPreparedDesktopStream();
    clearQrStream();
    sourceMode.value = "device";
    deviceRefreshToken.value += 1;
};

const useQrTimeSource = () => {
    prepareQrStream();
};

const updateQrFrameRate = (frameRate: QRCodeFrameRate) => {
    selectQrFrameRate.value = frameRate;
    setSearchParams({ qrfps: frameRate.toString() });
    if (sourceMode.value !== "qrtime" || qrState.value !== "previewing") {
        return;
    }

    clearQrStream();
    const inputStream = ensureQrInputStream();
    if (!inputStream) {
        setLogs("QRCode Time stream initialization failed.");
        return;
    }

    updatePreviewStream(inputStream);
    qrState.value = "previewing";
    setLogs(`QR source updated to ${frameRate} fps.`);
};

watchEffect(() => {
    const frameRate = parseQRCodeFrameRate(searchParams.qrfps);
    if (sourceMode.value === "qrtime" && qrState.value === "publishing") {
        return;
    }
    if (frameRate !== selectQrFrameRate.value) {
        updateQrFrameRate(frameRate);
    }
});

const startDisabled = computed(
    () =>
        disabled.value ||
        (sourceMode.value === "qrtime" && qrState.value !== "previewing") ||
        (sourceMode.value === "desktop" && !preparedDesktopStream.value),
);

const onAudioCodecChange = (event: Event) => {
    setSearchParams({ acodec: (event.target as HTMLSelectElement).value });
};

const onVideoCodecChange = (event: Event) => {
    setSearchParams({ vcodec: (event.target as HTMLSelectElement).value });
};

const onVideoWidthChange = (event: Event) => {
    selectVideoWidth.value = (event.target as HTMLSelectElement).value;
};

const onVideoHeightChange = (event: Event) => {
    selectVideoHeight.value = (event.target as HTMLSelectElement).value;
};

const onVideoLayerChange = (event: Event) => {
    selectVideoLayer.value = (event.target as HTMLSelectElement).value;
};

const onQrFrameRateChange = (event: Event) => {
    updateQrFrameRate(
        parseQRCodeFrameRate((event.currentTarget as HTMLSelectElement).value),
    );
};
</script>

<template>
    <legend>WHIP</legend>
    <div style="text-align: center">
        <canvas ref="qrCanvasRef" style="display: none" />

        <section
            style="margin-bottom: 0.6rem; display: flex; justify-content: center; gap: 0.5rem; flex-wrap: wrap"
        >
            <button type="button" :disabled="disabled" @click="useDeviceSource">
                Use Device
            </button>
            <button type="button" :disabled="disabled" @click="prepareDesktopStream">
                Share Desktop
            </button>
            <button type="button" :disabled="disabled" @click="useQrTimeSource">
                QRCode Time
            </button>
        </section>

        <section>
            <span>Mode: {{ sourceMode }}</span>
        </section>

        <section v-if="sourceMode === 'device'">
            <Device
                :disabled="disabled"
                :refresh-token="deviceRefreshToken"
                @select-audio="selectAudioDevice = $event"
                @select-video="selectVideoDevice = $event"
            />
        </section>

        <section>
            <label>
                Audio Codec:
                <select
                    :value="searchParams.acodec || ''"
                    :disabled="disabled"
                    @change="onAudioCodecChange"
                >
                    <option v-for="o in AudioCodecOptions" :key="o.value" :value="o.value">
                        {{ o.text }}
                    </option>
                </select>
            </label>
            <label>
                Video Codec:
                <select
                    :value="selectedVideoCodec"
                    :disabled="disabled"
                    @change="onVideoCodecChange"
                >
                    <option v-for="o in VideoCodecOptions" :key="o.value" :value="o.value">
                        {{ o.text }}
                    </option>
                </select>
            </label>
        </section>
        <section>
            <label>
                Video Width:
                <select :disabled="disabled" @change="onVideoWidthChange">
                    <option v-for="o in VideoWidthOptions" :key="o.value" :value="o.value">
                        {{ o.text }}
                    </option>
                </select>
            </label>
            <label>
                Video Height:
                <select :disabled="disabled" @change="onVideoHeightChange">
                    <option v-for="o in VideoHeightOptions" :key="o.value" :value="o.value">
                        {{ o.text }}
                    </option>
                </select>
            </label>
        </section>
        <section>
            <input
                v-model="selectAudioPseudo"
                type="checkbox"
                :disabled="disabled || sourceMode !== 'device'"
            />
            Use Pseudo Audio Track
        </section>
        <section>
            <label>
                SVC Layer:
                <select :disabled="disabled" @change="onVideoLayerChange">
                    <option v-for="o in WhipLayerOptions" :key="o.value" :value="o.value">
                        {{ o.text }}
                    </option>
                </select>
            </label>
        </section>
        <section>
            <button type="button" :disabled="startDisabled" @click="start">
                Start
            </button>
            <button type="button" :disabled="!disabled" @click="stopPublishing">
                Stop
            </button>
        </section>
        <section v-if="sourceMode === 'desktop' && !disabled">
            <small>{{
                preparedDesktopStream
                    ? "Desktop source ready. Click Start to publish."
                    : "Click Share Desktop to choose a screen, window, or tab."
            }}</small>
        </section>
        <template v-if="sourceMode === 'qrtime' && !disabled">
            <section>
                <label>
                    QR Frame Rate:
                    <select :value="selectQrFrameRate" @change="onQrFrameRateChange">
                        <option
                            v-for="frameRate in QRCodeFrameRates"
                            :key="frameRate"
                            :value="frameRate"
                        >
                            {{ frameRate }} fps
                        </option>
                    </select>
                </label>
            </section>
            <section>
                <small>{{
                    qrState === "previewing"
                        ? "QR source ready. Click Start to publish."
                        : qrState === "publishing"
                          ? "QR source is publishing."
                          : "Click QRCode Time to generate a QR preview."
                }}</small>
            </section>
            <section>
                <span>QR State: {{ qrState }}</span>
            </section>
        </template>
        <section>
            <h3>WHIP Video:</h3>
            <h5>
                Audio Track Count: {{ audioTrackCount }}, Video Track Count:
                {{ videoTrackCount }}
            </h5>
            <h5 v-if="sourceMode === 'qrtime'">
                QR Target FPS: {{ selectQrFrameRate }}
            </h5>
            <Player
                v-if="stream"
                :stream="stream"
                :show-render-fps="sourceMode === 'qrtime'"
            />
        </section>
        <section>
            <Datachannel v-if="datachannel" :datachannel="datachannel" />
        </section>
        <section>
            <h4>Logs:</h4>
            <pre>{{ logs.join("\n") }}</pre>
        </section>
    </div>
</template>
