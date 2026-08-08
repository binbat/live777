<script setup lang="ts">
import type { StatsNerds } from "player-core";

interface Props {
    stats: StatsNerds;
}

defineProps<Props>();

const emit = defineEmits<{
    close: [];
}>();

const formatBytes = (bytes: number): string => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${parseFloat((bytes / k ** i).toFixed(2))} ${sizes[i]}`;
};

const convertSecondsToMilliseconds = (seconds: number): number => {
    return seconds ? Math.round(seconds * 1000) : 0;
};
</script>

<template>
    <div class="title">
        <div>Stats for nerds</div>
        <span class="btn" @click="emit('close')">[X]</span>
    </div>

    <dl class="stats">
        <dt>Received:</dt>
        <dd>{{ formatBytes(stats.bytesReceived) }}</dd>

        <dt>Sent:</dt>
        <dd>{{ formatBytes(stats.bytesSent) }}</dd>

        <dt>Round Trip Time:</dt>
        <dd>{{ convertSecondsToMilliseconds(stats.currentRoundTripTime) }}ms</dd>

        <dt>Video Codec:</dt>
        <dd>{{ stats.vcodec ?? "-" }}</dd>

        <dt>Audio Codec:</dt>
        <dd>{{ stats.acodec ?? "-" }}</dd>

        <dt>Video Resolution:</dt>
        <dd>
            {{
                `${stats.frameWidth}x${stats.frameHeight}@${stats.framesPerSecond}`
            }}
        </dd>

        <dt>Audio volume:</dt>
        <dd>{{ stats.muted ? "muted" : stats.audioLevel?.toFixed(2) }}</dd>
    </dl>
</template>
