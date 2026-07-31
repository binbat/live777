<script lang="ts">
export interface ICascadeDialog {
    show(streamId: string): void;
}
</script>

<script setup lang="ts">
import { ref, useTemplateRef } from "vue";

import { cascade } from "../api";

interface Props {
    /** pull: create a stream sourced from a remote WHEP URL;
     * push: relay an existing stream to a remote WHIP URL. */
    mode: "pull" | "push";
}

const props = defineProps<Props>();

const streamId = ref("");
const cascadeURL = ref("");
const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");

const show = (id: string) => {
    streamId.value = id;
    cascadeURL.value = props.mode === "pull"
        ? `${location.origin}/whep/`
        : `${location.origin}/whip/push`;
    dialogEl.value?.showModal();
};

defineExpose({ show });

const onConfirmCascadeURL = () => {
    if (cascadeURL.value === "") {
        return;
    }
    if (props.mode === "pull") {
        void cascade(streamId.value, { sourceUrl: cascadeURL.value });
    } else {
        void cascade(streamId.value, { targetUrl: cascadeURL.value });
    }
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="true" class="modal">
        <div class="modal-box max-w-md">
            <div class="w-full text-xl mb-2">
                <h3 v-if="mode === 'pull'" class="font-bold">Cascade Pull</h3>
                <h3 v-else class="font-bold">Cascade Push ({{ streamId }})</h3>
            </div>
            <div>
                <div v-if="mode === 'pull'" class="form-control">
                    <label class="label px-0">Stream ID:</label>
                    <input v-model="streamId" class="input input-bordered" />
                </div>
                <div class="form-control">
                    <label class="label px-0">{{ mode === "pull" ? "Source URL:" : "Target URL:" }}</label>
                    <input v-model="cascadeURL" class="input input-bordered" />
                </div>
            </div>
            <div class="modal-action">
                <form method="dialog" class="flex gap-2">
                    <button class="btn" @click="onConfirmCascadeURL">Confirm</button>
                    <button class="btn">Cancel</button>
                </form>
            </div>
        </div>
    </dialog>
</template>
