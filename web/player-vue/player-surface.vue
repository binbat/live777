<script setup lang="ts">
import { collectWebRtcStats, type StatsNerds } from "player-core";
import { onMounted, onUnmounted, ref, useTemplateRef, watchEffect } from "vue";
import StatsForNerds from "./stats-for-nerds.vue";

interface Props {
    stream?: MediaStream | null;
    autoplay?: boolean;
    muted?: boolean;
    controls?: boolean;
    getPeerConnection?: () => RTCPeerConnection | null;
}

const props = withDefaults(defineProps<Props>(), {
    stream: null,
    autoplay: false,
    muted: false,
    controls: false,
});

const emit = defineEmits<{
    click: [event: MouseEvent];
    "video-element": [video: HTMLVideoElement];
}>();

const videoEl = useTemplateRef("videoEl");
const statsNerds = ref<StatsNerds | null>(null);
let statsInterval: ReturnType<typeof setInterval> | null = null;

const tryPlay = () => {
    const video = videoEl.value;
    if (!props.autoplay || !video || !video.srcObject) return;
    video.muted = props.muted;
    video.play().catch(() => {
        // Autoplay can be blocked if the caller does not mute the video.
    });
};

watchEffect(
    (onCleanup) => {
        const video = videoEl.value;
        if (!video) return;
        video.srcObject = props.stream ?? null;
        queueMicrotask(tryPlay);
        if (!props.stream) return;
        video.addEventListener("loadedmetadata", tryPlay);
        video.addEventListener("canplay", tryPlay);
        onCleanup(() => {
            video.removeEventListener("loadedmetadata", tryPlay);
            video.removeEventListener("canplay", tryPlay);
        });
    },
    { flush: "post" },
);

const stopSyncStats = () => {
    if (statsInterval) {
        clearInterval(statsInterval);
        statsInterval = null;
    }
    statsNerds.value = null;
};

const syncStats = async () => {
    const peerConnection = props.getPeerConnection?.();
    if (!peerConnection) return;

    const stats = await collectWebRtcStats(peerConnection);
    stats.muted = videoEl.value?.muted;
    statsNerds.value = stats;
};

const startSyncStats = () => {
    if (statsInterval) return;
    void syncStats();
    statsInterval = setInterval(syncStats, 1000);
};

onMounted(() => {
    const video = videoEl.value;
    if (!video) return;
    emit("video-element", video);
    video.addEventListener("contextmenu", startSyncStats);
});

onUnmounted(() => {
    videoEl.value?.removeEventListener("contextmenu", startSyncStats);
    stopSyncStats();
});
</script>

<template>
    <div id="player" class="player-wrapper">
        <video
            ref="videoEl"
            :autoplay="autoplay"
            :muted="muted"
            :controls="controls"
            playsinline
            @click="emit('click', $event)"
        />
        <div v-if="statsNerds" id="stats" class="stats-container">
            <StatsForNerds :stats="statsNerds" @close="stopSyncStats" />
        </div>
    </div>
</template>
