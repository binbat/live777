<script setup lang="ts">
import { PlayerSurface } from "@binbat/whep-player-vue";
import { onUnmounted, ref } from "vue";

const DisplayWidthOptions = [
    { value: "320px", text: "320px" },
    { value: "480px", text: "480px" },
    { value: "600px", text: "600px" },
    { value: "1280px", text: "1280px" },
    { value: "1920px", text: "1920px" },
    { value: "", text: "auto" },
];

const props = withDefaults(
    defineProps<{
        stream: MediaStream;
        showRenderFps?: boolean;
        getPeerConnection?: () => RTCPeerConnection | null;
    }>(),
    {
        showRenderFps: false,
        getPeerConnection: undefined,
    },
);

const emit = defineEmits<{
    "video-element": [video: HTMLVideoElement];
}>();

const resolution = ref("");
const renderFps = ref<number | null>(null);
const displayWidth = ref("320px");

let videoEl: HTMLVideoElement | null = null;
let renderFpsCallbackId: number | null = null;
let renderFpsToken = 0;
let windowPresentedFrames: number | null = null;
let windowStartedAt: DOMHighResTimeStamp | null = null;
let lastFrameTime: DOMHighResTimeStamp | null = null;
let staleTimer: ReturnType<typeof setInterval> | null = null;

const handleResize = () => {
    if (!videoEl) return;
    resolution.value = `${videoEl.videoWidth}x${videoEl.videoHeight}`;
};

const stopRenderFps = () => {
    renderFpsToken += 1;
    if (videoEl && renderFpsCallbackId !== null) {
        videoEl.cancelVideoFrameCallback(renderFpsCallbackId);
    }
    renderFpsCallbackId = null;
    windowPresentedFrames = null;
    windowStartedAt = null;
    lastFrameTime = null;
    if (staleTimer) {
        clearInterval(staleTimer);
        staleTimer = null;
    }
    renderFps.value = null;
};

const startRenderFps = () => {
    if (!videoEl || !props.showRenderFps) return;
    stopRenderFps();
    const video = videoEl;
    const token = renderFpsToken;

    const onFrame: VideoFrameRequestCallback = (now, metadata) => {
        if (token !== renderFpsToken) return;
        if (windowPresentedFrames === null || windowStartedAt === null) {
            windowPresentedFrames = metadata.presentedFrames;
            windowStartedAt = now;
        } else if (
            metadata.presentedFrames > windowPresentedFrames &&
            now - windowStartedAt >= 1000
        ) {
            const frameCount = metadata.presentedFrames - windowPresentedFrames;
            renderFps.value = (frameCount * 1000) / (now - windowStartedAt);
            windowPresentedFrames = metadata.presentedFrames;
            windowStartedAt = now;
        }
        lastFrameTime = now;
        renderFpsCallbackId = video.requestVideoFrameCallback(onFrame);
    };

    renderFpsCallbackId = video.requestVideoFrameCallback(onFrame);
    staleTimer = setInterval(() => {
        if (
            lastFrameTime !== null &&
            performance.now() - lastFrameTime > 2000
        ) {
            windowPresentedFrames = null;
            windowStartedAt = null;
            renderFps.value = null;
        }
    }, 1000);
};

const onVideoElement = (video: HTMLVideoElement) => {
    if (videoEl === video) return;
    stopRenderFps();
    videoEl?.removeEventListener("resize", handleResize);
    videoEl = video;
    emit("video-element", video);
    video.addEventListener("resize", handleResize);
    handleResize();
    startRenderFps();
};

onUnmounted(() => {
    stopRenderFps();
    videoEl?.removeEventListener("resize", handleResize);
});
</script>

<template>
    <h5>
        Raw Resolution: {{ resolution
        }}<template v-if="showRenderFps"
            >@{{ renderFps?.toFixed(1) ?? "--" }}</template
        >
    </h5>
    <label>
        Video Width:
        <select v-model="displayWidth">
            <option v-for="o in DisplayWidthOptions" :key="o.value" :value="o.value">
                {{ o.text }}
            </option>
        </select>
    </label>
    <br />
    <div :style="{ width: displayWidth, margin: '0 auto' }">
        <PlayerSurface
            :stream="stream"
            autoplay
            muted
            controls
            :get-peer-connection="getPeerConnection"
            @video-element="onVideoElement"
        />
    </div>
</template>
