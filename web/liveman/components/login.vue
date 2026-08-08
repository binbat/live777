<script setup lang="ts">
import { ref, useTemplateRef, watch } from "vue";
import { WretchError } from "wretch/resolver";

import * as livemanApi from "../api";
import * as sharedApi from "@/shared/api";
import { formatHttpError } from "@/shared/utils";

enum AuthorizeType {
    Password = "Password",
    Token = "Token"
}

const authorizeTypes = Object.values(AuthorizeType);

interface Props {
    show: boolean;
}

const props = defineProps<Props>();

const emit = defineEmits<{
    success: [tokenType: string, tokenValue: string];
}>();

const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");
const authType = ref(AuthorizeType.Password);
const username = ref("");
const password = ref("");
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

const handleLogin = async () => {
    errMsg.value = null;
    loading.value = true;
    try {
        let tokenType, tokenValue;
        switch (authType.value) {
            case AuthorizeType.Password: {
                const res = await livemanApi.login(username.value, password.value);
                tokenType = res.token_type;
                tokenValue = res.access_token;
                break;
            }
            case AuthorizeType.Token: {
                tokenType = "Bearer";
                tokenValue = token.value;
                livemanApi.setAuthToken(`${tokenType} ${tokenValue}`);
                await livemanApi.getNodes();
                break;
            }
        }
        const tk = `${tokenType} ${tokenValue}`;
        livemanApi.setAuthToken(tk);
        sharedApi.setAuthToken(tk);
        emit("success", tokenType, tokenValue);
    } catch (e) {
        livemanApi.setAuthToken("");
        sharedApi.setAuthToken("");
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
            <div class="w-full text-xl mb-2">
                <h3 class="font-bold">Authorization Required</h3>
            </div>
            <div role="tablist" class="tabs tabs-bordered tabs-lg my-4">
                <a
                    v-for="t in authorizeTypes"
                    :key="t"
                    role="tab"
                    class="tab text-base"
                    :class="{ 'tab-active': t === authType }"
                    @click="authType = t"
                >{{ t }}</a>
            </div>
            <div v-if="typeof errMsg === 'string'" role="alert" class="alert alert-error">{{ errMsg }}</div>
            <form @submit.prevent="handleLogin">
                <template v-if="authType === AuthorizeType.Password">
                    <label class="input input-bordered flex items-center gap-2 my-4">
                        <span>Username</span>
                        <input v-model="username" type="text" class="grow" name="Username" />
                    </label>
                    <label class="input input-bordered flex items-center gap-2 my-4">
                        <span>Password</span>
                        <input v-model="password" type="password" class="grow" name="Password" />
                    </label>
                </template>
                <label v-else-if="authType === AuthorizeType.Token" class="input input-bordered flex items-center gap-2 my-4">
                    <span>Token</span>
                    <input v-model="token" type="text" class="grow" name="Token" />
                </label>
                <button type="submit" class="btn btn-primary w-full text-base" :disabled="loading">
                    <span v-if="loading" class="loading loading-spinner loading-sm"></span>
                    <span>Login</span>
                </button>
            </form>
        </div>
    </dialog>
</template>
