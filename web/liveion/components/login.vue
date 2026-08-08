<script setup lang="ts">
import { ref, useTemplateRef, watch } from "vue";
import { WretchError } from "wretch/resolver";

import * as api from "@/shared/api";
import { formatHttpError } from "@/shared/utils";

interface Props {
    show: boolean;
}

const props = defineProps<Props>();

const emit = defineEmits<{
    success: [token: string];
}>();

const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");
const token = ref("");
const loading = ref(false);
const errMsg = ref<string | null>(null);

watch(() => props.show, (show) => {
    const dialog = dialogEl.value;
    if (!dialog) {
        return;
    }
    if (show && !dialog.open) {
        dialog.showModal();
    } else if (!show && dialog.open) {
        dialog.close();
    }
});

const handleDialogClose = () => {
    if (props.show) {
        dialogEl.value?.showModal();
    }
};

const onTokenSubmit = async () => {
    loading.value = true;
    const tk = token.value.indexOf(" ") < 0 ? `Bearer ${token.value}` : token.value;
    api.setAuthToken(tk);
    try {
        await api.getStreams();
        emit("success", token.value);
    } catch (e) {
        api.setAuthToken("");
        if (e instanceof WretchError) {
            errMsg.value = formatHttpError(e);
        } else if (e instanceof Error) {
            errMsg.value = e.message;
        } else {
            errMsg.value = String(e);
        }
    }
    loading.value = false;
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="true" class="modal" @close="handleDialogClose">
        <div class="modal-box">
            <div class="w-full mb-8 text-xl">
                <h3 class="font-bold">Authorization Required</h3>
            </div>
            <div v-if="typeof errMsg === 'string'" role="alert" class="alert alert-error">{{ errMsg }}</div>
            <form @submit.prevent="onTokenSubmit">
                <label class="input input-bordered flex items-center gap-2 my-4">
                    <span>Token</span>
                    <input v-model="token" class="grow" />
                </label>
                <button type="submit" class="btn btn-primary w-full text-base" :disabled="loading">
                    <span v-if="loading" class="loading loading-spinner loading-sm"></span>
                    <span>Login</span>
                </button>
            </form>
        </div>
    </dialog>
</template>
