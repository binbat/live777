<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { RefreshCw, Calendar, Search, Play, Link2, Copy } from "lucide-vue-next";

import * as api from "../api";
import { useToken } from "../context";

function formatYearMonthDay(timestamp: string): string {
    const date = new Date(parseInt(timestamp) * 1000);
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, "0");
    const day = String(date.getDate()).padStart(2, "0");

    return `${year}-${month}-${day}`;
}

function formatDateTime(timestamp: string): string {
    const date = new Date(parseInt(timestamp) * 1000);
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, "0");
    const day = String(date.getDate()).padStart(2, "0");
    const hours = String(date.getHours()).padStart(2, "0");
    const minutes = String(date.getMinutes()).padStart(2, "0");
    const seconds = date.getSeconds().toString().padStart(2, "0");

    return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}`;
}

function formatDuration(durationMs?: number | null): string {
    if (durationMs == null || durationMs <= 0) {
        return "--:--";
    }

    const totalSeconds = Math.floor(durationMs / 1000);
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = totalSeconds % 60;

    if (hours > 0) {
        return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
    }

    return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function getFileName(path: string) {
    try {
        const idx = path.lastIndexOf("/");
        return idx >= 0 ? path.slice(idx + 1) : path;
    } catch {
        return path;
    }
}

const token = useToken();
const streams = ref<string[]>([]);
const streamFilter = ref("");
const selectedStream = ref<string>("");
const indexEntries = ref<api.RecordingIndexEntry[]>([]);
const loading = ref(true);
const error = ref<string>("");

const fetchStreams = async () => {
    try {
        loading.value = true;
        error.value = "";
        const res = await api.getRecordingIndexStreams();
        streams.value = res;
        if (!selectedStream.value && res.length > 0) {
            selectedStream.value = res[0];
        }
    } catch {
        error.value = "Failed to fetch streams";
    } finally {
        loading.value = false;
    }
};

const fetchIndex = async () => {
    if (!selectedStream.value) {
        indexEntries.value = [];
        return;
    }
    try {
        loading.value = true;
        error.value = "";
        const res = await api.getRecordingIndexByStream(selectedStream.value);
        res.sort((a, b) => parseInt(b.record) - parseInt(a.record));
        indexEntries.value = res;
    } catch {
        error.value = "Failed to fetch recording index";
    } finally {
        loading.value = false;
    }
};

onMounted(() => {
    void fetchStreams();
    void fetchIndex();
});

// The original re-ran both fetchers whenever selectedStream changed (the
// useCallback identities depended on it), e.g. after fetchStreams picked
// the first stream.
watch(selectedStream, () => {
    void fetchStreams();
    void fetchIndex();
});

const playMpd = (mpd: string) => {
    const params = new URLSearchParams();
    params.set("mpd", mpd);
    if (token.value) params.set("token", token.value);
    const url = new URL(`/tools/dash.html?${params.toString()}`, location.origin);
    window.open(url.toString(), "_blank");
};

const copyToClipboard = async (text: string) => {
    try { await navigator.clipboard.writeText(text); } catch { /* ignore */ }
};

const dashPlayerLink = (mpdPath: string) => new URL(
    `/tools/dash.html?mpd=${encodeURIComponent(mpdPath)}${token.value ? `&token=${encodeURIComponent(token.value)}` : ""}`,
    location.origin
).toString();

const mpdUrl = (mpdPath: string) => new URL(api.getSegmentUrl(mpdPath), location.origin).toString();

const filteredStreams = computed(() => {
    const f = streamFilter.value.trim().toLowerCase();
    if (!f) return streams.value;
    return streams.value.filter(s => s.toLowerCase().includes(f));
});

const groupedByDay = computed(() => {
    const groups = new Map<string, api.RecordingIndexEntry[]>();
    for (const e of indexEntries.value) {
        const key = formatYearMonthDay(e.record);
        const arr = groups.get(key) ?? [];
        arr.push(e);
        groups.set(key, arr);
    }
    // sort each group by day desc
    for (const [, arr] of groups) arr.sort((a, b) => parseInt(b.record) - parseInt(a.record));
    // return sorted keys desc by year-month
    return Array.from(groups.entries()).sort((a, b) => b[0].localeCompare(a[0]));
});
</script>

<template>
    <div v-if="loading && streams.length === 0" class="flex justify-center items-center h-64">
        <span class="loading loading-spinner loading-lg"></span>
    </div>
    <div v-else class="space-y-6">
        <!-- Header -->
        <div class="flex items-center gap-2 px-4 h-12">
            <span class="font-bold text-lg">Recordings</span>
            <div aria-label="Badge" class="badge badge-ghost font-bold mr-auto">{{ indexEntries.length }}</div>
            <button class="btn btn-sm btn-ghost gap-2" @click="void fetchStreams(); void fetchIndex();">
                Refresh
                <RefreshCw class="size-4" />
            </button>
        </div>

        <!-- Stream picker -->
        <div class="flex flex-wrap items-center gap-3 px-4">
            <div class="relative">
                <input
                    v-model="streamFilter"
                    class="input input-sm input-bordered focus:outline-offset-0 pl-8"
                    placeholder="Search streams"
                />
                <Search class="w-4 h-4 absolute left-2 top-1/2 -translate-y-1/2 text-gray-500" />
            </div>
            <select v-model="selectedStream" class="select select-sm select-bordered focus:outline-offset-0">
                <option v-for="s in filteredStreams" :key="s" :value="s">{{ s }}</option>
            </select>
        </div>

        <div v-if="error" class="alert alert-error">
            <span>{{ error }}</span>
        </div>

        <!-- Empty state -->
        <div v-if="selectedStream && indexEntries.length === 0" aria-label="Card" class="card card-bordered p-8">
            <div class="text-center text-gray-500">
                <Calendar class="w-16 h-16 mx-auto mb-4 opacity-50" />
                <p class="text-lg mb-2">No recordings</p>
                <p class="text-sm">Recordings for the selected stream will appear here.</p>
            </div>
        </div>

        <!-- Grouped list -->
        <div v-for="[ymd, list] in groupedByDay" :key="ymd" aria-label="Card" class="card card-bordered p-4">
            <div class="flex items-center justify-between mb-3">
                <div class="flex items-center gap-2">
                    <span class="text-lg font-semibold">{{ ymd }}</span>
                    <div aria-label="Badge" class="badge badge-ghost">{{ list.length }}</div>
                </div>
                <span class="text-sm opacity-70 truncate">{{ selectedStream }}</span>
            </div>
            <div class="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
                <div v-for="e in list" :key="e.record" class="border border-base-200 rounded-lg p-3 flex flex-col gap-2">
                    <div class="flex items-center justify-between">
                        <div class="min-w-0">
                            <div class="font-medium">{{ formatDateTime(e.record) }}</div>
                            <div class="text-xs opacity-70">Duration {{ formatDuration(e.duration_ms) }}</div>
                        </div>
                        <span class="text-xs opacity-70 font-mono truncate ml-3" :title="e.mpd_path">
                            {{ getFileName(e.mpd_path) }}
                        </span>
                    </div>
                    <div class="flex items-center gap-2">
                        <button class="btn btn-sm btn-primary flex-1" @click="playMpd(e.mpd_path)">
                            <Play class="w-4 h-4" />
                            Play
                        </button>
                        <div role="tooltip" data-tip="Copy DASH player link" class="tooltip">
                            <button class="btn btn-sm btn-ghost" @click="copyToClipboard(dashPlayerLink(e.mpd_path))">
                                <Link2 class="w-4 h-4" />
                            </button>
                        </div>
                        <div role="tooltip" data-tip="Copy MPD URL" class="tooltip">
                            <button class="btn btn-sm btn-ghost" @click="copyToClipboard(mpdUrl(e.mpd_path))">
                                <Copy class="w-4 h-4" />
                            </button>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </div>
</template>
