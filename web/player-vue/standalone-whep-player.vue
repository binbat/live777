<script setup lang="ts">
import WhepPlayer from "./whep-player.vue";

// Full-page standalone player driven by URL query params, mirroring the
// retired Solid alone-player page:
//   ?id=<stream>&token=<token>&autoplay&muted&controls&reconnect=<ms>
//   &lowlatency — pin the receiver jitter buffer to zero (playoutDelayHint=0)
//   &input — remote input capture (wayland source with input=1)
const params = new URLSearchParams(location.search);
const streamId = params.get("id") ?? "";
const token = params.get("token") ?? "";
const autoplay = params.has("autoplay");
const controls = params.has("controls");
const muted = params.has("muted");
const lowLatency = params.has("lowlatency");
const input = params.has("input");
const parsed = Number.parseInt(params.get("reconnect") ?? "0", 10);
const reconnectMs = Number.isNaN(parsed) ? 0 : parsed;
const url = `${location.origin}/whep/${streamId}`;
</script>

<template>
    <WhepPlayer
        :url="url"
        :token="token"
        :autoplay="autoplay"
        :muted="muted"
        :controls="controls"
        :reconnect-ms="reconnectMs"
        :low-latency="lowLatency"
        :input="input"
    />
</template>
