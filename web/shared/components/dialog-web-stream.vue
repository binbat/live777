<script lang="ts">
export interface IWebStreamDialog {
    show(streamId: string): void;
}
</script>

<script setup lang="ts">
import { onMounted, onUnmounted, ref, shallowRef, useTemplateRef } from "vue";
import { WHIPClient } from "@binbat/whip-whep/whip";

import { useToken } from "../context";
import { formatVideoTrackResolution } from "../utils";
import { useLogger } from "../hooks/use-logger";
import {
    DefaultQRCodeFrameRate,
    type QRCodeFrameRate,
    QRCodeFrameRates,
    QRCodeStream
} from "../qrcode-stream";
import convertSessionDescription from "../sdp-codec";

interface Props {
    getWhipUrl?: (streamId: string) => string;
}

const props = defineProps<Props>();

const emit = defineEmits<{
    stop: [];
}>();

const streamId = ref("");
const token = useToken();
let mediaStream: MediaStream | null = null;
const whipClient = shallowRef<WHIPClient | null>(null);
const connState = ref("");
const videoResolution = ref("");
const { logs, log, clear } = useLogger();
const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");
const videoEl = useTemplateRef<HTMLVideoElement>("videoEl");
const canvasEl = useTemplateRef<HTMLCanvasElement>("canvasEl");
let qrCodeStream: QRCodeStream | null = null;
const qrPreviewing = ref(false);
const qrSourceHidden = ref(false);
let qrSourceHiddenLogged = false;
const qrFrameRate = ref<QRCodeFrameRate>(DefaultQRCodeFrameRate);

const show = (id: string) => {
    streamId.value = id;
    dialogEl.value?.showModal();
};

defineExpose({ show });

const handleCloseDialog = () => {
    dialogEl.value?.close();
};

const updateConnState = (state: string) => {
    connState.value = state;
    log(state);
};

const handleVisibilityChange = () => {
    if (
        document.visibilityState !== "hidden" ||
        !qrCodeStream ||
        !whipClient.value
    ) {
        return;
    }
    qrSourceHidden.value = true;
    if (!qrSourceHiddenLogged) {
        log("QR source page is hidden; browser timer throttling may reduce QR fps.");
        qrSourceHiddenLogged = true;
    }
};

onMounted(() => {
    document.addEventListener("visibilitychange", handleVisibilityChange);
});

onUnmounted(() => {
    document.removeEventListener("visibilitychange", handleVisibilityChange);
});

const handleStreamStart = async (stream: MediaStream) => {
    clear();
    connState.value = "";
    mediaStream = stream;
    if (videoEl.value) {
        videoEl.value.srcObject = stream;
    }
    updateConnState("Started");
    const pc = new RTCPeerConnection();
    pc.addEventListener("iceconnectionstatechange", () => {
        updateConnState(pc.iceConnectionState);
    });
    stream.getVideoTracks().forEach(vt => {
        pc.addTransceiver(vt, { direction: "sendonly" });
        videoResolution.value = formatVideoTrackResolution(vt);
    });
    stream.getAudioTracks().forEach(at => {
        pc.addTransceiver(at, { direction: "sendonly" });
    });
    const whip = new WHIPClient();
    whipClient.value = whip;
    whip.onOffer = sdp => {
        log("http offer sent");
        try {
            return convertSessionDescription(sdp, "", "h264");
        } catch (error) {
            const message = error instanceof Error ? error.message : "H264 is not available";
            log(message);
            throw error;
        }
    };
    whip.onAnswer = sdp => {
        log("http answer received");
        return sdp;
    };
    try {
        const url = props.getWhipUrl?.(streamId.value) ?? `${location.origin}/whip/${streamId.value}`;
        await whip.publish(pc, url, token.value);
    } catch (e: any) {  // eslint-disable-line @typescript-eslint/no-explicit-any
        connState.value = "Error";
        if (e instanceof Error) {
            log(e.message);
        }
        const r = e.response as Response | undefined;
        if (r) {
            log(await r.text());
        }
    }
};

const handleDisplayMediaStart = async () => {
    const stream = await navigator.mediaDevices.getDisplayMedia({
        audio: true,
        video: true
    });
    void handleStreamStart(stream);
};

const handleEncodeLatencyPreview = () => {
    qrSourceHidden.value = false;
    qrSourceHiddenLogged = false;
    if (!qrCodeStream) {
        qrCodeStream = new QRCodeStream(
            canvasEl.value!,
            qrFrameRate.value
        );
    }
    const stream = qrCodeStream.capture();
    mediaStream = stream;
    if (videoEl.value) {
        videoEl.value.srcObject = stream;
    }
    qrPreviewing.value = true;
};

const updateQrFrameRate = (frameRate: QRCodeFrameRate) => {
    qrFrameRate.value = frameRate;
    if (!qrPreviewing.value || whipClient.value) {
        return;
    }

    if (qrCodeStream) {
        qrCodeStream.stop();
        qrCodeStream = null;
    }
    qrCodeStream = new QRCodeStream(
        canvasEl.value!,
        frameRate
    );
    const stream = qrCodeStream.capture();
    mediaStream = stream;
    if (videoEl.value) {
        videoEl.value.srcObject = stream;
    }
    log(`QR source updated to ${frameRate} fps.`);
};

const handleQrFrameRateChange = (e: Event) => {
    updateQrFrameRate(
        Number.parseInt(
            (e.currentTarget as HTMLSelectElement).value,
            10
        ) as QRCodeFrameRate
    );
};

const handleEncodeLatencyPublish = () => {
    if (mediaStream) {
        void handleStreamStart(mediaStream);
        log("For QR latency testing, keep this source page visible and use Preview -> Decode Latency on this page.");
        qrPreviewing.value = false;
    }
};

const handleStreamStop = async () => {
    qrPreviewing.value = false;
    qrSourceHidden.value = false;
    qrSourceHiddenLogged = false;
    if (qrCodeStream) {
        qrCodeStream.stop();
        qrCodeStream = null;
    }
    if (mediaStream) {
        mediaStream.getTracks().forEach(t => t.stop());
        mediaStream = null;
    }
    if (videoEl.value) {
        videoEl.value.srcObject = null;
    }
    if (whipClient.value) {
        await whipClient.value.stop();
        whipClient.value = null;
    }
    emit("stop");
    handleCloseDialog();
};

const handleVideoResize = () => {
    const videoTrack = mediaStream?.getVideoTracks()[0];
    if (videoTrack) {
        videoResolution.value = formatVideoTrackResolution(videoTrack);
    }
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="false" class="modal">
        <div class="modal-box min-w-md max-w-[unset] w-[unset]">
            <div class="w-full text-xl mb-6">
                <h3 class="font-bold">Web Stream {{ streamId }} {{ videoResolution }}</h3>
            </div>
            <div>
                <video
                    ref="videoEl"
                    class="block mx-auto min-w-[28rem] max-w-[90vw] max-h-[70vh]"
                    controls
                    autoplay
                    @resize="handleVideoResize"
                ></video>
                <details class="collapse collapse-arrow text-sm">
                    <summary class="collapse-title px-0">
                        <b>Connection Status: </b>
                        <code>{{ connState }}</code>
                    </summary>
                    <div class="collapse-content px-0">
                        <pre class="overflow-auto max-h-[10lh]">{{ logs.join("\n") }}</pre>
                    </div>
                </details>
                <div v-if="qrSourceHidden" class="alert alert-warning mt-2">
                    <span>QR source page was hidden; browser timer throttling may reduce QR fps.</span>
                </div>
            </div>
            <div class="modal-action mt-0">
                <label class="mr-auto">
                    QR Frame Rate:
                    <select
                        :disabled="!!whipClient"
                        :value="qrFrameRate"
                        @change="handleQrFrameRateChange"
                    >
                        <option v-for="frameRate in QRCodeFrameRates" :key="frameRate" :value="frameRate">
                            {{ frameRate }} fps
                        </option>
                    </select>
                </label>
                <button v-if="whipClient" class="btn btn-error" @click="handleStreamStop">Stop</button>
                <template v-else-if="qrPreviewing">
                    <button class="btn btn-success" @click="handleEncodeLatencyPublish">Publish</button>
                    <button class="btn btn-error" @click="handleStreamStop">Stop</button>
                </template>
                <template v-else>
                    <button class="btn btn-info" @click="handleEncodeLatencyPreview">Encode Latency</button>
                    <button class="btn" @click="handleDisplayMediaStart">Start</button>
                </template>
                <button class="btn" @click="handleCloseDialog">Hide</button>
            </div>
            <canvas ref="canvasEl" class="hidden" width="1280" height="720"></canvas>
        </div>
    </dialog>
</template>
