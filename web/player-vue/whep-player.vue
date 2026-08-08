<script setup lang="ts">
import { watch } from "vue";
import PlayerSurface from "./player-surface.vue";
import { useWhepPlayback } from "./use-whep-playback";

interface Props {
    url: string;
    token?: string;
    autoplay?: boolean;
    muted?: boolean;
    controls?: boolean;
    reconnectMs?: number;
    createDataChannel?: boolean;
    lowLatency?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
    token: "",
    autoplay: false,
    muted: false,
    controls: false,
    reconnectMs: undefined,
    createDataChannel: false,
    lowLatency: false,
});

const playback = useWhepPlayback({
    url: () => props.url,
    token: () => props.token,
    reconnectMs: () => props.reconnectMs,
    createDataChannel: props.createDataChannel,
    lowLatency: () => props.lowLatency,
});

const { stream, peerConnection, status } = playback;

let videoEl: HTMLVideoElement | null = null;

const onVideoElement = (video: HTMLVideoElement) => {
    videoEl = video;
};

const playVideoElement = () => {
    videoEl?.play().catch(() => {
        // Ignore autoplay rejection; click-to-play still works.
    });
};

const onVideoClick = async (event: MouseEvent) => {
    if (peerConnection.value) return;
    event.preventDefault();
    await playback.play();
    playVideoElement();
};

watch(
    () => [props.autoplay, props.url],
    () => {
        if (!props.autoplay || !props.url) return;
        void playback.play();
    },
    { immediate: true },
);

watch([status, stream], () => {
    if (status.value !== "playing" || !stream.value) return;
    playVideoElement();
});

defineExpose({
    play: playback.play,
    stop: playback.stop,
    mute: playback.mute,
    selectLayer: playback.selectLayer,
});
</script>

<template>
    <PlayerSurface
        :stream="stream"
        :autoplay="autoplay"
        :muted="muted"
        :controls="controls"
        :get-peer-connection="() => peerConnection"
        @click="onVideoClick"
        @video-element="onVideoElement"
    />
</template>
