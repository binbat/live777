<script lang="ts">
export interface IBitrateDialog {
    show(streamId: string): void;
}
</script>

<script setup lang="ts">
import { computed, ref, useTemplateRef } from "vue";

import {
    type SourceBitrateStatus,
    clearSourceBitrate,
    getSourceBitrate,
    setSourceBitrate,
} from "../api";
import { formatBitrate } from "../utils";

const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");

const streamId = ref("");
const status = ref<SourceBitrateStatus | null>(null);
const loading = ref(false);
const busy = ref(false);
const noSource = ref(false);
const errorMessage = ref("");
const manualInput = ref("");

const MODE_LABELS: Record<string, string> = {
    adaptive: "Adaptive",
    manual: "Manual",
    fixed: "Fixed",
};

const show = (id: string) => {
    streamId.value = id;
    status.value = null;
    noSource.value = false;
    errorMessage.value = "";
    manualInput.value = "";
    dialogEl.value?.showModal();
    void refresh();
};

defineExpose({ show });

// backend error bodies are plain text; wretch exposes them as `text`
const describeError = (error: unknown): string => {
    const e = error as { status?: number; text?: unknown; message?: unknown };
    const text = typeof e?.text === "string" ? e.text.trim() : "";
    const fallback = typeof e?.message === "string" && e.message.length > 0 ? e.message : "Unknown error";
    if (typeof e?.status === "number") {
        return text.length > 0 ? `HTTP ${e.status}: ${text}` : `HTTP ${e.status}`;
    }
    return text.length > 0 ? text : fallback;
};

const refresh = async () => {
    loading.value = true;
    errorMessage.value = "";
    noSource.value = false;
    try {
        status.value = await getSourceBitrate(streamId.value);
    } catch (error: unknown) {
        status.value = null;
        if ((error as { status?: number })?.status === 404) {
            noSource.value = true;
        } else {
            errorMessage.value = describeError(error);
        }
    } finally {
        loading.value = false;
    }
};

const runAction = async (action: () => Promise<unknown>) => {
    busy.value = true;
    errorMessage.value = "";
    try {
        await action();
        await refresh();
    } catch (error: unknown) {
        errorMessage.value = describeError(error);
    } finally {
        busy.value = false;
    }
};

const handleSelectTier = (tier: string) => runAction(() => setSourceBitrate(streamId.value, { tier }));

const manualBitrate = computed(() => {
    const value = Number(manualInput.value);
    return Number.isInteger(value) && value > 0 ? value : null;
});

const handleApplyManual = () => {
    const bitrate = manualBitrate.value;
    if (bitrate === null) {
        errorMessage.value = "Bitrate must be a positive integer (bps)";
        return;
    }
    void runAction(() => setSourceBitrate(streamId.value, { bitrate }));
};

const handleClearOverride = () => runAction(() => clearSourceBitrate(streamId.value));
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="true" class="modal">
        <div class="modal-box max-w-md">
            <div class="w-full text-xl mb-2">
                <h3 class="font-bold">Source Bitrate ({{ streamId }})</h3>
            </div>
            <div v-if="loading && !status" class="py-4 text-sm opacity-70">Loading…</div>
            <div v-else-if="noSource" class="py-4 text-sm opacity-70">
                This stream has no adjustable encoder source.
            </div>
            <template v-else-if="status">
                <div class="flex items-center gap-2 py-1 text-sm">
                    <span
                        class="badge"
                        :class="{
                            'badge-info': status.mode === 'adaptive',
                            'badge-warning': status.mode === 'manual',
                            'badge-ghost': status.mode === 'fixed',
                        }"
                    >{{ MODE_LABELS[status.mode] ?? status.mode }}</span>
                    <span v-if="status.bitrate !== null" class="font-mono">{{ formatBitrate(status.bitrate) }}</span>
                    <span v-else class="opacity-70">bitrate unknown</span>
                    <span v-if="status.manual_bitrate !== null" class="opacity-70">
                        (override: {{ formatBitrate(status.manual_bitrate) }})
                    </span>
                </div>
                <div v-if="status.tiers.length > 0" class="form-control">
                    <label class="label px-0">Quality tiers:</label>
                    <div class="flex flex-wrap gap-1">
                        <button
                            v-for="tier in status.tiers"
                            :key="tier.name"
                            class="btn btn-sm"
                            :class="{ 'btn-info': status.active_tier === tier.name, 'btn-disabled': busy }"
                            :disabled="busy"
                            @click="handleSelectTier(tier.name)"
                        >{{ tier.name }} · {{ formatBitrate(tier.bitrate) }}</button>
                    </div>
                </div>
                <div class="form-control">
                    <label class="label px-0">Manual bitrate (bps):</label>
                    <div class="flex gap-2">
                        <input
                            v-model="manualInput"
                            type="number"
                            min="1"
                            step="1"
                            placeholder="e.g. 1500000"
                            class="input input-bordered input-sm flex-1"
                            @keyup.enter="handleApplyManual"
                        />
                        <button
                            class="btn btn-sm btn-primary"
                            :class="{ 'btn-disabled': busy || manualBitrate === null }"
                            :disabled="busy || manualBitrate === null"
                            @click="handleApplyManual"
                        >Apply</button>
                    </div>
                </div>
                <div v-if="status.manual_bitrate !== null" class="pt-2">
                    <button
                        class="btn btn-sm btn-warning"
                        :class="{ 'btn-disabled': busy }"
                        :disabled="busy"
                        @click="handleClearOverride"
                    >{{ status.adaptive ? "Resume adaptive control" : "Clear manual override" }}</button>
                </div>
            </template>
            <div v-if="errorMessage" class="alert alert-error my-2">
                <span>{{ errorMessage }}</span>
            </div>
            <div class="modal-action">
                <form method="dialog">
                    <button class="btn">Close</button>
                </form>
            </div>
        </div>
    </dialog>
</template>
