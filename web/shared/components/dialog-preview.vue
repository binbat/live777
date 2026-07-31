<script lang="ts">
export interface IPreviewDialog {
    show(streamId: string): void;
}
</script>

<script setup lang="ts">
import { ref, useTemplateRef } from "vue";
import { ClockIcon } from "@heroicons/vue/24/outline";
import { WHEPClient } from "@binbat/whip-whep/whep.js";

import { useToken } from "../context";
import { formatVideoTrackResolution } from "../utils";
import { useLogger } from "../hooks/use-logger";
import { QRCodeStreamDecoder } from "../qrcode-stream";

interface Props {
    getWhepUrl?: (streamId: string) => string;
}

const props = defineProps<Props>();

const emit = defineEmits<{
    stop: [];
}>();

const streamId = ref("");
const token = useToken();
let peerConnection: RTCPeerConnection | null = null;
let whepClient: WHEPClient | null = null;
let mediaStream: MediaStream | null = null;
const connState = ref("");
const videoResolution = ref("");
let videoResolutionInterval = -1;
const { logs, log, clear } = useLogger();
const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");
const videoEl = useTemplateRef<HTMLVideoElement>("videoEl");
let streamDecoder: QRCodeStreamDecoder | null = null;
const latency = ref<string>();

const show = async (newStreamId: string) => {
    if (streamId.value !== newStreamId) {
        if (streamId.value !== "" && whepClient !== null) {
            await handlePreviewStop();
        }
        streamId.value = newStreamId;
        void handlePreviewStart(newStreamId);
    }
    dialogEl.value?.showModal();
};

defineExpose({ show });

const handleCloseDialog = () => {
    const activeElement = document.activeElement;
    if (activeElement instanceof HTMLElement && dialogEl.value?.contains(activeElement)) {
        activeElement.blur();
    }
    dialogEl.value?.close();
};

const handlePreviewStop = async () => {
    window.clearInterval(videoResolutionInterval);
    if (streamDecoder) {
        streamDecoder.stop();
        streamDecoder = null;
    }
    if (videoEl.value) {
        videoEl.value.srcObject = null;
    }
    mediaStream = null;
    peerConnection = null;
    if (whepClient) {
        await whepClient.stop();
        whepClient = null;
    }
    emit("stop");
    handleCloseDialog();
};

const updateConnState = (state: string) => {
    connState.value = state;
    log(state);
};

const logInboundRtpStats = async () => {
    const stats = await peerConnection?.getStats() ?? null;
    if (!stats) return;
    for (const [, s] of stats) {
        if (s.type === "inbound-rtp") {
            const { id, bytesReceived } = s as RTCInboundRtpStreamStats;
            // log the first time bytesReceived is not 0
            if (bytesReceived) {
                log(`inbound-rtp(${id}): ${bytesReceived} bytes`);
                return;
            }
        }
    }
    window.queueMicrotask(logInboundRtpStats);
};

const handlePreviewStart = async (id: string) => {
    clear();
    updateConnState("Started");
    const pc = new RTCPeerConnection();
    pc.addTransceiver("video", { direction: "recvonly" });
    pc.addTransceiver("audio", { direction: "recvonly" });
    const ms = new MediaStream();
    mediaStream = ms;
    if (videoEl.value) {
        videoEl.value.srcObject = ms;
    }
    pc.addEventListener("track", ev => {
        log(`track: ${ev.track.kind}`);
        ms.addTrack(ev.track);
    });
    pc.addEventListener("iceconnectionstatechange", () => {
        const state = pc.iceConnectionState;
        updateConnState(state);
        if (state === "connected") {
            window.queueMicrotask(logInboundRtpStats);
        }
    });
    peerConnection = pc;
    const whep = new WHEPClient();
    whep.onOffer = sdp => {
        log("http offer sent");
        return sdp;
    };
    whep.onAnswer = sdp => {
        log("http answer received");
        return sdp;
    };
    whepClient = whep;
    try {
        const url = props.getWhepUrl?.(id) ?? `${location.origin}/whep/${id}`;
        await whep.view(pc, url, token.value);
    } catch (e: any) { // eslint-disable-line @typescript-eslint/no-explicit-any
        connState.value = "Error";
        if (e instanceof Error) {
            log(e.message);
        }
        const r = e.response as Response | undefined;
        if (r) {
            log(await r.text());
        }
    }
    if (videoResolutionInterval >= 0) {
        window.clearInterval(videoResolutionInterval);
        videoResolutionInterval = -1;
    }
    videoResolutionInterval = window.setInterval(refreshVideoResolution, 1000);
};

const handleVideoCanPlay = () => {
    log("video canplay");
};

const refreshVideoResolution = () => {
    if (mediaStream) {
        const videoTrack = mediaStream.getVideoTracks()[0];
        if (videoTrack) {
            videoResolution.value = formatVideoTrackResolution(videoTrack);
        }
    }
};

const handleDecodeLatency = (e: Event) => {
    e.preventDefault();
    latency.value = "-- ms";
    if (videoEl.value != null && streamDecoder == null) {
        streamDecoder = new QRCodeStreamDecoder(videoEl.value);
    }
    const decoder = streamDecoder!;
    decoder.start();
    decoder.addEventListener("latency", (e: CustomEvent<number>) => {
        latency.value = `${e.detail} ms`;
    });
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="false" class="modal">
        <div class="modal-box min-w-md max-w-[unset] w-[unset]">
            <div class="w-full text-xl mb-6">
                <h3 class="font-bold">Preview {{ streamId }} {{ videoResolution }}</h3>
            </div>
            <div>
                <video
                    ref="videoEl"
                    class="mx-[-1.5rem] min-w-[28rem] max-w-[90vw] max-h-[70vh]"
                    controls
                    autoplay
                    @canplay="handleVideoCanPlay"
                    @resize="refreshVideoResolution"
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
            </div>
            <div class="modal-action mt-0">
                <div class="mr-auto">
                    <button
                        v-if="typeof latency === 'string'"
                        class="btn btn-ghost gap-2 font-normal no-animation"
                    >
                        <ClockIcon class="size-5 stroke-current" />
                        Latency: {{ latency }}
                    </button>
                    <button v-else class="btn btn-info" @click="handleDecodeLatency">Decode Latency</button>
                </div>
                <button class="btn btn-error" @click="handlePreviewStop">Stop</button>
                <button class="btn" @click="handleCloseDialog">Hide</button>
            </div>
        </div>
    </dialog>
</template>
