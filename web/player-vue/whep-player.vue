<script setup lang="ts">
import { useTemplateRef, watch } from "vue";
import { attachInputCapture } from "player-core";
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
    input?: boolean;
    lowLatency?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
    token: "",
    autoplay: false,
    muted: false,
    controls: false,
    reconnectMs: undefined,
    createDataChannel: false,
    input: false,
    lowLatency: false,
});

const playback = useWhepPlayback({
    url: () => props.url,
    token: () => props.token,
    reconnectMs: () => props.reconnectMs,
    createDataChannel: props.createDataChannel,
    input: props.input,
    lowLatency: () => props.lowLatency,
});

const { stream, peerConnection, inputChannel, status } = playback;

let videoEl: HTMLVideoElement | null = null;
const surfaceRef = useTemplateRef("surfaceRef");

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

// Remote input (wayland source with input=1): attach pointer/keyboard
// capture to the player surface while the "input" datachannel exists.
let detachInput: (() => void) | null = null;
watch([inputChannel], () => {
    detachInput?.();
    detachInput = null;
    const channel = inputChannel.value;
    const el = surfaceRef.value?.$el as HTMLElement | undefined;
    if (!props.input || !channel || !el) return;
    detachInput = attachInputCapture({
        target: el,
        video: () => videoEl,
        channel: () => inputChannel.value,
    });
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
        ref="surfaceRef"
        :stream="stream"
        :autoplay="autoplay"
        :muted="muted"
        :controls="controls"
        :get-peer-connection="() => peerConnection"
        @click="onVideoClick"
        @video-element="onVideoElement"
    />
</template>
