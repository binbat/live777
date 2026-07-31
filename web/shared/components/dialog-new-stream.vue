<script lang="ts">
export interface INewStreamDialog {
    show(initialId: string): void;
}
</script>

<script setup lang="ts">
import { ref, useTemplateRef } from "vue";

import { createStream } from "../api";

const emit = defineEmits<{
    "new-stream-id": [id: string];
    "stream-created": [];
}>();

const streamId = ref("");
const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");

const show = (initialId: string) => {
    streamId.value = initialId;
    dialogEl.value?.showModal();
};

defineExpose({ show });

function getCurrentNode(): string | null {
    const urlParams = new URLSearchParams(window.location.search);
    return urlParams.get("nodes");
}

async function handleCreateStream(id: string) {
    const currentNode = getCurrentNode();
    try {
        await createStream(id, currentNode);
        return true;
    } catch (error: unknown) {
        if (
            error instanceof Object &&
            "response" in error &&
            typeof (error as { response: { status: number } }).response.status === "number" &&
            (error as { response: { status: number } }).response.status === 409
        ) {
            window.alert("Resource already exists, please use a different streamId");
            return false;
        }
        window.alert("Failed to create stream, please try again later");
        return false;
    }
}

const closeDialog = () => {
    const activeElement = document.activeElement;
    if (activeElement instanceof HTMLElement && dialogEl.value?.contains(activeElement)) {
        activeElement.blur();
    }
    dialogEl.value?.close();
};

const onConfirmNewStreamId = async () => {
    if (!streamId.value.trim()) {
        window.alert("Please enter a valid Stream ID");
        return;
    }

    const success = await handleCreateStream(streamId.value);
    if (success) {
        closeDialog();
        emit("new-stream-id", streamId.value);
        emit("stream-created");
    }
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="false" class="modal">
        <div class="modal-box max-w-md">
            <div class="w-full text-xl mb-2">
                <h3 class="font-bold">New Stream</h3>
            </div>
            <div>
                <div class="form-control">
                    <label class="label px-0">Stream ID:</label>
                    <input v-model="streamId" class="input input-bordered" />
                </div>
            </div>
            <div class="modal-action">
                <form method="dialog" class="flex gap-2">
                    <button type="button" class="btn" @click="onConfirmNewStreamId">Confirm</button>
                    <button type="button" class="btn" @click="closeDialog">Cancel</button>
                </form>
            </div>
        </div>
    </dialog>
</template>
