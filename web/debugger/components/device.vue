<script setup lang="ts">
import { ref, watch } from "vue";

const NoneDevice = { value: "", text: "none" };

function deviceInfoToOption(info: MediaDeviceInfo) {
    const value = info.deviceId;
    let text = info.label;
    if (text.length <= 0) {
        text = `${info.kind} (${info.deviceId})`;
    }
    return { value, text };
}

function uniqByValue<T extends { value: unknown }>(items: T[]) {
    const map = new Map<unknown, T>();
    for (const item of items) {
        if (!map.has(item.value)) {
            map.set(item.value, item);
        }
    }
    return Array.from(map.values());
}

const props = defineProps<{
    disabled: boolean;
    refreshToken: number;
}>();

const emit = defineEmits<{
    selectAudio: [deviceId: string];
    selectVideo: [deviceId: string];
}>();

const audioDevices = ref([NoneDevice]);
const videoDevices = ref([NoneDevice]);

const refreshDevice = async () => {
    try {
        // to obtain non-empty device label, there needs to be an active media stream or persistent permission
        // https://developer.mozilla.org/en-US/docs/Web/API/MediaDeviceInfo/label#value
        const mediaStream = await navigator.mediaDevices.getUserMedia({
            audio: true,
            video: true,
        });
        const devices = (
            await navigator.mediaDevices.enumerateDevices()
        ).filter((i) => !!i.deviceId);
        mediaStream.getTracks().map((track) => track.stop());
        const audio = devices
            .filter((i) => i.kind === "audioinput")
            .map(deviceInfoToOption);
        if (audio.length > 0) {
            audioDevices.value = uniqByValue(audio);
        }
        const video = devices
            .filter((i) => i.kind === "videoinput")
            .map(deviceInfoToOption);
        if (video.length > 0) {
            videoDevices.value = uniqByValue(video);
        }
    } catch (e) {
        console.error("refreshDevice failed:", e);
    }
};

watch(
    () => props.refreshToken,
    () => {
        void refreshDevice();
    },
);
watch(audioDevices, (items) => {
    if (items.length > 0) {
        emit("selectAudio", items[0].value);
    }
});
watch(videoDevices, (items) => {
    if (items.length > 0) {
        emit("selectVideo", items[0].value);
    }
});
</script>

<template>
    <div style="margin: 0.2rem">
        Audio Device:
        <select
            :disabled="disabled"
            @change="emit('selectAudio', ($event.target as HTMLSelectElement).value)"
        >
            <option v-for="d in audioDevices" :key="d.value" :value="d.value">
                {{ d.text }}
            </option>
        </select>
    </div>
    <div style="margin: 0.2rem">
        Video Device:
        <select
            :disabled="disabled"
            @change="emit('selectVideo', ($event.target as HTMLSelectElement).value)"
        >
            <option v-for="d in videoDevices" :key="d.value" :value="d.value">
                {{ d.text }}
            </option>
        </select>
    </div>
</template>
