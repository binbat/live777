<script setup lang="ts">
import { onUnmounted, ref } from "vue";

const props = defineProps<{
    datachannel: RTCDataChannel;
}>();

const datachannelState = ref("");
const logs = ref<string[]>([]);

const onmessage = (ev: MessageEvent) => {
    logs.value = [
        ...logs.value,
        new TextDecoder("utf-8").decode(new Uint8Array(ev.data)),
    ];
};
const onopen = () => {
    datachannelState.value = "opened";
};
const onclose = () => {
    datachannelState.value = "closed";
};

props.datachannel.addEventListener("message", onmessage);
props.datachannel.addEventListener("close", onclose);
props.datachannel.addEventListener("open", onopen);

onUnmounted(() => {
    props.datachannel.removeEventListener("open", onopen);
    props.datachannel.removeEventListener("close", onclose);
    props.datachannel.removeEventListener("message", onmessage);
});

const send = (event: Event) => {
    props.datachannel.send((event.target as HTMLInputElement).value);
};
</script>

<template>
    <div>State: {{ datachannelState }}</div>
    <div>
        Datachannel:
        <input type="text" @change="send" />
    </div>
    <pre>{{ logs.join("\n") }}</pre>
</template>
