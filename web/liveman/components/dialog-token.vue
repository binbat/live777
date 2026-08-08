<script lang="ts">
export interface IStreamTokenDialog {
    show(id: string): void;
}
</script>

<script setup lang="ts">
import { ref, useTemplateRef, watch } from "vue";

import { createStreamToken } from "../api";

enum StreamTokenPermission {
    Sub = "subscribe",
    Pub = "publish",
    Admin = "admin"
}

const streamTokenPermissions = Object.values(StreamTokenPermission);

const streamId = ref("");
const duration = ref(3600);
const permissions = ref<Record<StreamTokenPermission, boolean>>({
    subscribe: true,
    publish: false,
    admin: false
});
const token = ref("");
const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");
const tokenResultEl = useTemplateRef<HTMLInputElement>("tokenResultEl");

const show = (id: string) => {
    streamId.value = id;
    dialogEl.value?.showModal();
};

defineExpose({ show });

const onConfirm = async () => {
    const res = await createStreamToken({
        id: streamId.value,
        duration: duration.value,
        ...permissions.value
    });
    token.value = `${res.token_type} ${res.access_token}`;
};

// the input mounts in the same render pass that set the token, so
// flush: "post" (Preact effects ran after render)
watch(token, (t) => {
    if (t) {
        tokenResultEl.value?.focus();
        tokenResultEl.value?.select();
    }
}, { flush: "post" });

const onClose = () => {
    token.value = "";
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="true" class="modal" @close="onClose">
        <div class="modal-box max-w-md">
            <div class="w-full text-xl mb-2">
                <h3 class="font-bold">Create Token for stream {{ streamId }}</h3>
            </div>
            <div>
                <label class="form-control">
                    <label class="label px-0">Duration:</label>
                    <label class="input input-bordered flex items-center gap-2">
                        <input
                            class="grow"
                            type="number"
                            :value="duration"
                            @input="duration = ($event.target as HTMLInputElement).valueAsNumber"
                        />
                        <span>Seconds</span>
                    </label>
                </label>
                <label class="form-control mt-4">
                    <label class="label px-0 pb-0">Permissions:</label>
                    <label v-for="p in streamTokenPermissions" :key="p" class="label justify-start gap-2">
                        <input
                            type="checkbox"
                            class="checkbox checkbox-xs"
                            :name="p"
                            :checked="permissions[p]"
                            @change="permissions = { ...permissions, [p]: ($event.target as HTMLInputElement).checked }"
                        />
                        <span>{{ p }}</span>
                    </label>
                </label>
            </div>
            <div class="modal-action">
                <form method="dialog" class="flex gap-2">
                    <button class="btn" @click.prevent="onConfirm">Confirm</button>
                    <button class="btn">Cancel</button>
                </form>
            </div>
            <label v-if="token" class="form-control">
                <label class="label px-0">Token:</label>
                <input ref="tokenResultEl" class="input input-bordered" type="text" :value="token" />
            </label>
        </div>
    </dialog>
</template>
