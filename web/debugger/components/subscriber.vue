<script setup lang="ts">
import { useWhepPlayback } from "@binbat/whep-player-vue";
import { computed, onUnmounted, ref, watch, watchEffect } from "vue";
import {
    DefaultQRCodeFrameRate,
    parseQRCodeFrameRate,
    type QRCodeFrameRate,
    QRCodeStreamDecoder,
} from "../../shared/qrcode-stream";
import { useLogger } from "../primitive/logger";
import { useSearchParams } from "../primitive/search-params";
import Datachannel from "./datachannel.vue";
import Player from "./player.vue";
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

const [searchParams, setSearchParams] = useSearchParams();

const disabled = ref(true);
const disabledAudio = ref(false);
const disabledVideo = ref(false);
// ?lowlatency pre-enables the no-jitter-buffer mode; toggling the checkbox
// writes the param back so the link stays shareable.
const lowLatency = ref(searchParams.lowlatency !== undefined);
const [logs, setLogs, clear] = useLogger();

const isMeasuringQrLatency = ref(false);
const measurementStartedAt = ref<number | null>(null);
const latestLatencySample = ref<QrLatencySample | null>(null);
const freshnessNow = ref(0);
const expectedQrFrameRate = ref<QRCodeFrameRate>(
    parseQRCodeFrameRate(searchParams.qrfps ?? DefaultQRCodeFrameRate),
);

let videoEl: HTMLVideoElement | null = null;
let decoder: QRCodeStreamDecoder | null = null;
let freshnessTimer: number | null = null;

const latencyDisplay = computed(() => {
    const startedAt = measurementStartedAt.value;
    if (startedAt === null) {
        return null;
    }
    return getQrLatencyDisplay(
        freshnessNow.value,
        startedAt,
        latestLatencySample.value,
    );
});

const playback = useWhepPlayback({
    url: () => {
        const streamId = (searchParams.id || "").trim();
        return `${location.origin}/whep/${encodeURIComponent(streamId)}`;
    },
    token: () => searchParams.token || "",
    createDataChannel: true,
    lowLatency,
    log: setLogs,
});

const {
    stream,
    peerConnection,
    datachannel,
    audioTrackCount,
    videoTrackCount,
} = playback;

onUnmounted(() => {
    stopQrLatencyMeasure();
    void playback.stop({ reconnect: false });
});

watch(stream, (currentStream) => {
    if (!currentStream) {
        stopQrLatencyMeasure();
    }
});

watch(lowLatency, (enabled) => {
    setSearchParams({ lowlatency: enabled ? "1" : null });
});

watchEffect(() => {
    const frameRate = parseQRCodeFrameRate(
        searchParams.qrfps ?? DefaultQRCodeFrameRate,
    );
    if (frameRate !== expectedQrFrameRate.value) {
        expectedQrFrameRate.value = frameRate;
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
    isMeasuringQrLatency.value = false;
    measurementStartedAt.value = null;
    latestLatencySample.value = null;
    freshnessNow.value = 0;
}

function startQrLatencyMeasure() {
    if (!videoEl || !stream.value) {
        return;
    }
    stopQrLatencyMeasure();
    const startedAt = performance.now();
    isMeasuringQrLatency.value = true;
    measurementStartedAt.value = startedAt;
    freshnessNow.value = startedAt;
    freshnessTimer = window.setInterval(() => {
        freshnessNow.value = performance.now();
    }, 250);
    decoder = new QRCodeStreamDecoder(videoEl);
    decoder.addEventListener("latency", (e: CustomEvent<number>) => {
        const receivedAt = performance.now();
        latestLatencySample.value = {
            latencyMs: e.detail,
            receivedAtMs: receivedAt,
        };
        freshnessNow.value = receivedAt;
    });
    decoder.start();
}

const start = async () => {
    clear();
    stopQrLatencyMeasure();
    const streamId = (searchParams.id || "").trim();
    if (!streamId) {
        setLogs("Stream ID is required before subscribing.");
        return;
    }
    await playback.play();
    disabled.value = false;
};

const stop = () => {
    stopQrLatencyMeasure();
    void playback.stop({ reconnect: false });
    disabled.value = true;
};

const toggleAudio = () => {
    const wasDisabled = disabledAudio.value;
    disabledAudio.value = !wasDisabled;
    void playback.mute({
        kind: "audio",
        enabled: wasDisabled,
    });
};

const toggleVideo = () => {
    const wasDisabled = disabledVideo.value;
    disabledVideo.value = !wasDisabled;
    void playback.mute({
        kind: "video",
        enabled: wasDisabled,
    });
};

const onSelectLayer = (event: Event) => {
    void playback.selectLayer((event.target as HTMLSelectElement).value);
};

const getPeerConnection = () => peerConnection.value;

const onVideoElement = (video: HTMLVideoElement) => {
    videoEl = video;
};
</script>

<template>
    <legend>WHEP</legend>
    <div style="text-align: center">
        <section>
            SVC Layer:
            <select :disabled="disabled" @change="onSelectLayer">
                <option v-for="o in WhepLayerOptions" :key="o.value" :value="o.value">
                    {{ o.text }}
                </option>
            </select>
        </section>
        <section>
            <label
                title="Disable the jitter buffer so decoded frames render immediately (lower latency, helps high-fps playback such as 120fps)"
            >
                <input v-model="lowLatency" type="checkbox" />
                Low Latency (No Jitter Buffer)
            </label>
        </section>
        <section>
            <button type="button" :disabled="disabled" @click="toggleAudio">
                {{ disabledAudio ? "Enable" : "Disable" }} Audio
            </button>
            <button type="button" :disabled="disabled" @click="toggleVideo">
                {{ disabledVideo ? "Enable" : "Disable" }} Video
            </button>
        </section>
        <section>
            <button type="button" :disabled="!disabled" @click="start">Start</button>
            <button type="button" :disabled="disabled" @click="stop">Stop</button>
        </section>

        <section>
            <button
                type="button"
                :disabled="disabled || !stream || isMeasuringQrLatency"
                @click="startQrLatencyMeasure"
            >
                Measure QR Latency
            </button>
            <button
                type="button"
                :disabled="!isMeasuringQrLatency"
                @click="stopQrLatencyMeasure"
            >
                Stop Measuring
            </button>
        </section>

        <section>
            <h3>WHEP Video:</h3>
            <h5>
                Audio Track Count: {{ audioTrackCount }}, Video Track Count:
                {{ videoTrackCount }}
            </h5>
            <h5>
                QR Target FPS: {{ expectedQrFrameRate
                }}<span
                    v-if="latencyDisplay"
                    :class="`qr-latency qr-latency-${latencyDisplay.state}`"
                    >{{ " | Latency: " }}{{ latencyDisplay.value }} |
                    {{ latencyDisplay.status }}</span
                >
            </h5>
            <Player
                v-if="stream"
                :stream="stream"
                show-render-fps
                :get-peer-connection="getPeerConnection"
                @video-element="onVideoElement"
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
