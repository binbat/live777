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

const formatBytesRate = (bytesPerSecond?: number): string => {
    if (bytesPerSecond === undefined) return "-";
    if (bytesPerSecond >= 1_000_000)
        return `${(bytesPerSecond / 1_000_000).toFixed(1)} MB/s`;
    if (bytesPerSecond >= 1_000)
        return `${Math.round(bytesPerSecond / 1_000)} KB/s`;
    return `${Math.round(bytesPerSecond)} B/s`;
};

const convertSecondsToMilliseconds = (seconds: number): number => {
    return seconds ? Math.round(seconds * 1000) : 0;
};

const formatJitter = (jitter?: number): string => {
    if (jitter === undefined) return "-";
    return `${(jitter * 1000).toFixed(1)} ms`;
};

const formatPacketLoss = (lost?: number, received?: number): string => {
    if (lost === undefined) return "-";
    const total = (received ?? 0) + lost;
    const ratio = total > 0 ? ((lost / total) * 100).toFixed(1) : "0.0";
    return `${lost} / ${total} (${ratio}%)`;
};

const formatConcealment = (concealed?: number, samples?: number): string => {
    if (concealed === undefined) return "-";
    if (!samples) return `${concealed}`;
    return `${concealed} (${((concealed / samples) * 100).toFixed(1)}%)`;
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

        <dt>Download Speed:</dt>
        <dd>{{ formatBytesRate(stats.bytesReceivedPerSecond) }}</dd>

        <dt>Upload Speed:</dt>
        <dd>{{ formatBytesRate(stats.bytesSentPerSecond) }}</dd>

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

        <dt>Audio Jitter:</dt>
        <dd>{{ formatJitter(stats.audioJitter) }}</dd>

        <dt>Audio Packets Lost:</dt>
        <dd>{{ formatPacketLoss(stats.audioPacketsLost, stats.audioPacketsReceived) }}</dd>

        <dt>Audio Concealed:</dt>
        <dd>{{ formatConcealment(stats.audioConcealedSamples, stats.audioSamplesReceived) }}</dd>
    </dl>
</template>
