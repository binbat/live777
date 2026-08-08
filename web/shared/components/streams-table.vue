<script lang="ts">
import type { Stream } from "../api";

export interface StreamTableProps {
    getStreams?: () => Promise<Stream[]>;
    streamsSSEUrl?: string;
    getWhepUrl?: (streamId: string) => string;
    getWhipUrl?: (streamId: string) => string;
    showCascade?: boolean;
    features?: {
        player?: boolean;
        debugger?: boolean;
        recording?: boolean;
        autoDetectRecording?: boolean;
        recordingPlayback?: boolean;
    };
}
</script>

<script setup lang="ts">
import {
    computed,
    ref,
    toValue,
    useTemplateRef,
    watch,
    watchEffect,
    type ComputedRef,
    type MaybeRefOrGetter,
} from "vue";
import { ArrowPathIcon, ArrowRightEndOnRectangleIcon, PlusIcon } from "@heroicons/vue/24/outline";

import {
    type CapabilityProbeStatus,
    type SessionConnectionState,
    type Stream as StreamType,
    deleteStream,
    getRecordingStatus,
    getStreams,
    parseStreamsSSE,
    probeRecorderFeature,
    startRecording,
    stopRecording,
} from "../api";
import { formatBitrate, formatBytes, formatTime, nextSeqId } from "../utils";
import { useRefreshTimer } from "../hooks/use-refresh-timer";
import { useStreamSSE } from "../hooks/use-stream-sse";
import { useToken } from "../context";

import ClientsDialog, { type IClientsDialog } from "./dialog-clients.vue";
import CascadeDialog, { type ICascadeDialog } from "./dialog-cascade.vue";
import PreviewDialog, { type IPreviewDialog } from "./dialog-preview.vue";
import WebStreamDialog, { type IWebStreamDialog } from "./dialog-web-stream.vue";
import NewStreamDialog, { type INewStreamDialog } from "./dialog-new-stream.vue";

async function getStreamsSorted(): Promise<StreamType[]> {
    try {
        const streams = await getStreams();
        return streams.sort((a, b) => a.createdAt - b.createdAt);
    } catch {
        return [];
    }
}

const ACTIVE_STATES: SessionConnectionState[] = ["new", "connecting", "connected"];

function countActiveSessions(sessions: { state: SessionConnectionState }[]): number {
    return sessions.filter(s => ACTIVE_STATES.includes(s.state)).length;
}

type ConnectionStatus = "connected" | "connecting" | "reconnecting" | "disconnected" | "error";

interface StreamsDataSource {
    data: ComputedRef<StreamType[]>;
    isRefreshing: ComputedRef<boolean>;
    updateData: () => void | Promise<void>;
    toggleTimer: () => void;
    connectionStatus: ComputedRef<ConnectionStatus | undefined>;
}

function useStreamsDataSource(
    fetchStreams: () => Promise<StreamType[]>,
    streamsSSEUrl: MaybeRefOrGetter<string | undefined>,
): StreamsDataSource {
    const sseEnabled = ref(true);
    const token = useToken();
    const sse = useStreamSSE<StreamType[]>(
        {
            url: () => toValue(streamsSSEUrl) ?? null,
            token,
            parse: parseStreamsSSE,
            enabled: sseEnabled,
        },
        [],
    );
    const polling = useRefreshTimer<StreamType[]>([], fetchStreams);
    const isSSE = computed(() => !!toValue(streamsSSEUrl));

    const connectionStatus = computed<ConnectionStatus | undefined>(() => {
        if (!isSSE.value) {
            return undefined;
        }
        if (!sseEnabled.value) {
            return "disconnected";
        }
        if (sse.error.value) {
            return "error";
        }
        if (sse.connected.value) {
            return "connected";
        }
        if (sse.reconnecting.value) {
            return "reconnecting";
        }
        return "connecting";
    });

    return {
        data: computed(() => (isSSE.value ? sse.data.value : polling.data.value)),
        isRefreshing: computed(() => (isSSE.value ? sseEnabled.value : polling.isRefreshing.value)),
        updateData: () => (isSSE.value ? sse.reconnect() : polling.updateData()),
        toggleTimer: () => {
            if (isSSE.value) {
                sseEnabled.value = !sseEnabled.value;
            } else {
                polling.toggleTimer();
            }
        },
        connectionStatus,
    };
}

const props = defineProps<StreamTableProps>();

const token = useToken();
const {
    data: streamsData,
    isRefreshing,
    updateData,
    toggleTimer,
    connectionStatus,
} = useStreamsDataSource(props.getStreams ?? getStreamsSorted, () => props.streamsSSEUrl);

const selectedStreamId = ref("");
const cascadePullDialog = useTemplateRef<ICascadeDialog>("cascadePullDialog");
const cascadePushDialog = useTemplateRef<ICascadeDialog>("cascadePushDialog");
const clientsDialog = useTemplateRef<IClientsDialog>("clientsDialog");
const newStreamDialog = useTemplateRef<INewStreamDialog>("newStreamDialog");
const webStreams = ref<string[]>([]);
const newStreamId = ref("");
const webStreamDialogs = new Map<string, IWebStreamDialog>();
const previewStreams = ref<string[]>([]);
const previewStreamId = ref("");
const previewDialogs = new Map<string, IPreviewDialog>();

const setWebStreamDialog = (id: string, el: unknown) => {
    if (el) {
        webStreamDialogs.set(id, el as IWebStreamDialog);
    } else {
        webStreamDialogs.delete(id);
    }
};

const setPreviewDialog = (id: string, el: unknown) => {
    if (el) {
        previewDialogs.set(id, el as IPreviewDialog);
    } else {
        previewDialogs.delete(id);
    }
};

const features = computed(() => ({
    player: true,
    debugger: true,
    recording: true,
    autoDetectRecording: false,
    recordingPlayback: true,
    ...props.features,
}));

// refresh when the token changes; immediate matches the original Preact
// effect, which also ran once on mount (initial fetch for polling mode)
watch(token, () => {
    void updateData();
}, { immediate: true });

const recordingAvailable = ref(features.value.recording);
const recordingStates = ref<Record<string, boolean>>({});
const recordDialogOpen = ref(false);
const recordDialogStreamId = ref("");
const recordBusy = ref(false);
const recordError = ref("");
const recordMpd = ref("");
const confirmStopOpen = ref(false);
const confirmStopBusy = ref(false);

watchEffect((onCleanup) => {
    void token.value;
    if (!features.value.recording) {
        recordingAvailable.value = false;
        return;
    }

    if (!features.value.autoDetectRecording) {
        recordingAvailable.value = features.value.recording;
        return;
    }

    let disposed = false;
    onCleanup(() => {
        disposed = true;
    });
    (async () => {
        const status: CapabilityProbeStatus = await probeRecorderFeature();
        if (disposed) {
            return;
        }

        if (status === "available") {
            recordingAvailable.value = true;
        } else if (status === "unavailable") {
            recordingAvailable.value = false;
        } else {
            recordingAvailable.value = features.value.recording;
        }
    })();
});

watch([streamsData, recordingAvailable], () => {
    if (!recordingAvailable.value) {
        recordingStates.value = {};
        return;
    }

    // refresh recording status for visible streams
    (async () => {
        const states: Record<string, boolean> = {};
        for (const s of streamsData.value) {
            try {
                states[s.id] = await getRecordingStatus(s.id);
            } catch {
                states[s.id] = false;
            }
        }
        recordingStates.value = states;
    })();
}, { immediate: true });

const selectedStreamSessions = computed(
    () => streamsData.value.find(s => s.id == selectedStreamId.value)?.subscribe.sessions ?? []
);

const handleViewClients = (id: string) => {
    selectedStreamId.value = id;
    clientsDialog.value?.show();
};

const handleCascadePullStream = () => {
    const id = nextSeqId("pull-", streamsData.value.map(s => s.id));
    cascadePullDialog.value?.show(id);
};

const handleCascadePushStream = (id: string) => {
    cascadePushDialog.value?.show(id);
};

const handlePreview = (id: string) => {
    if (previewStreams.value.includes(id)) {
        previewDialogs.get(id)?.show(id);
    } else {
        previewStreams.value = [...previewStreams.value, id];
        previewStreamId.value = id;
    }
};

// the dialog for a freshly added preview mounts in the same render pass that
// triggered this watch, so flush: "post" (Preact effects ran after render)
watch(previewStreamId, id => {
    previewDialogs.get(id)?.show(id);
}, { flush: "post" });

const handlePreviewStop = (id: string) => {
    previewStreamId.value = "";
    previewStreams.value = previewStreams.value.filter(s => s !== id);
};

const handleNewStream = () => {
    const id = nextSeqId("web-", webStreams.value.concat(streamsData.value.map(s => s.id)));
    newStreamDialog.value?.show(id);
};

const handleNewStreamId = (id: string) => {
    webStreams.value = [...webStreams.value, id];
    newStreamId.value = id;
};

watch(newStreamId, id => {
    webStreamDialogs.get(id)?.show(id);
}, { flush: "post" });

const handleOpenWebStream = (id: string) => {
    webStreamDialogs.get(id)?.show(id);
};

const handleWebStreamStop = (id: string) => {
    newStreamId.value = "";
    webStreams.value = webStreams.value.filter(s => s !== id);
};

const handleOpenPlayerPage = (id: string) => {
    const params = new URLSearchParams();
    params.set("id", id);
    params.set("autoplay", "");
    params.set("muted", "");
    params.set("reconnect", "3000");
    params.set("token", token.value);
    const url = new URL(`/tools/player.html?${params.toString()}`, location.origin);
    window.open(url);
};

const handleOpenDebuggerPage = (id: string) => {
    const params = new URLSearchParams();
    params.set("id", id);
    params.set("token", token.value);
    const url = new URL(`/tools/debugger.html?${params.toString()}`, location.origin);
    window.open(url);
};

// Media statistics (issue #252): sum of all streams' rates and
// cumulative bytes. Liveman marks merged cluster snapshots as node-work
// totals because cascade hops are counted on each relay node.
const statsTotals = computed(() => streamsData.value.reduce(
    (acc, s) => ({
        rateIn: acc.rateIn + (s.stats?.publish.bitrate ?? 0),
        rateOut: acc.rateOut + (s.stats?.subscribe.bitrate ?? 0),
        bytesIn: acc.bytesIn + (s.stats?.publish.bytes ?? 0),
        bytesOut: acc.bytesOut + (s.stats?.subscribe.bytes ?? 0),
    }),
    { rateIn: 0, rateOut: 0, bytesIn: 0, bytesOut: 0 },
));
const statsLabel = computed(() => streamsData.value.some(s => s.statsScope === "clusterNodeWork") ? "Node work" : "Total");

const handleDestroyStream = async (id: string) => {
    await deleteStream(id);
    await updateData();
};

const openRecordDialog = (id: string) => {
    if (!recordingAvailable.value) {
        return;
    }
    if (recordingStates.value[id]) {
        recordDialogStreamId.value = id;
        confirmStopOpen.value = true;
        return;
    }
    recordDialogStreamId.value = id;
    recordError.value = "";
    recordMpd.value = "";
    recordDialogOpen.value = true;
};

const closeRecordDialog = () => {
    recordDialogOpen.value = false;
    recordBusy.value = false;
    recordError.value = "";
    recordMpd.value = "";
};

const handleConfirmRecord = async () => {
    if (!recordDialogStreamId.value) return;
    try {
        recordBusy.value = true;
        recordError.value = "";
        const res = await startRecording(recordDialogStreamId.value);
        recordingStates.value = { ...recordingStates.value, [recordDialogStreamId.value]: true };
        recordMpd.value = res.mpd_path;
    } catch (error: unknown) {
        const message = error instanceof Error ? error.message : "Failed to start recording";
        recordError.value = message;
    } finally {
        recordBusy.value = false;
    }
};

const handlePlayNow = () => {
    if (!features.value.recordingPlayback) return;
    if (!recordDialogStreamId.value || !recordMpd.value) return;
    const params = new URLSearchParams();
    params.set("mpd", recordMpd.value);
    if (token.value) params.set("token", token.value);
    const url = new URL(`/tools/dash.html?${params.toString()}`, location.origin);
    window.open(url.toString(), "_blank");
    closeRecordDialog();
};

const handleConfirmStop = async () => {
    confirmStopBusy.value = true;
    try {
        await stopRecording(recordDialogStreamId.value);
        recordingStates.value = { ...recordingStates.value, [recordDialogStreamId.value]: false };
    } finally {
        confirmStopBusy.value = false;
        confirmStopOpen.value = false;
        recordDialogStreamId.value = "";
    }
};

const handleCancelStop = () => {
    confirmStopOpen.value = false;
    recordDialogStreamId.value = "";
};
</script>

<template>
    <div class="flex items-center gap-2 px-4 h-12">
        <span class="font-bold text-lg">Streams</span>
        <div aria-label="Badge" class="badge badge-ghost font-bold mr-auto">{{ streamsData.length }}</div>
        <span
            class="text-sm opacity-70"
            :title="`${statsLabel} in ${formatBytes(statsTotals.bytesIn)} / out ${formatBytes(statsTotals.bytesOut)}`"
        >
            {{ statsLabel }}: {{ formatBitrate(statsTotals.rateIn) }} in · {{ formatBitrate(statsTotals.rateOut) }} out
        </span>
        <button v-if="showCascade" class="btn btn-sm btn-ghost gap-2" @click="handleCascadePullStream">
            <ArrowRightEndOnRectangleIcon class="size-4 stroke-current" />
            Cascade Pull
        </button>
        <span
            v-if="connectionStatus"
            :title="`SSE ${connectionStatus}`"
            class="inline-block size-2 rounded-full"
            :class="{
                'bg-success': connectionStatus === 'connected',
                'bg-warning animate-pulse': connectionStatus === 'connecting',
                'bg-base-300 animate-pulse': connectionStatus === 'reconnecting',
                'bg-error': connectionStatus === 'error',
                'bg-base-300': connectionStatus === 'disconnected',
            }"
        />
        <button class="btn btn-sm btn-ghost gap-2" @click="toggleTimer">
            Auto Refresh
            <input type="checkbox" class="checkbox checkbox-xs" :checked="isRefreshing" />
        </button>
        <button class="btn btn-sm btn-ghost gap-2" @click="updateData">
            Refresh
            <ArrowPathIcon class="size-4 stroke-current" />
        </button>
    </div>

    <table class="table overflow-x-auto">
        <thead>
            <tr>
                <th><span>ID</span></th>
                <td><span>Publisher</span></td>
                <td><span>Subscriber</span></td>
                <td><span>In</span></td>
                <td><span>Out</span></td>
                <td><span>Cascade</span></td>
                <td><span>Creation Time</span></td>
                <td><span>Operation</span></td>
            </tr>
        </thead>
        <tbody>
            <tr v-for="i in streamsData" :key="i.id">
                <th>
                    <span>
                        {{ i.id }}
                        <div
                            v-if="i.onDemand && countActiveSessions(i.publish.sessions) > 0"
                            aria-label="Badge"
                            class="badge badge-sm badge-info ml-2"
                        >on-demand</div>
                        <div
                            v-else-if="i.onDemand"
                            aria-label="Badge"
                            class="badge badge-sm badge-ghost ml-2"
                        >standby</div>
                        <div
                            v-if="!i.onDemand && i.provisioned"
                            aria-label="Badge"
                            class="badge badge-sm badge-ghost ml-2"
                        >config</div>
                    </span>
                </th>
                <td><span>{{ countActiveSessions(i.publish.sessions) }}</span></td>
                <td><span>{{ countActiveSessions(i.subscribe.sessions) }}</span></td>
                <td>
                    <span :title="`${formatBytes(i.stats?.publish.bytes ?? 0)} ${i.statsScope === 'clusterNodeWork' ? 'node work' : 'total'}`">
                        {{ formatBitrate(i.stats?.publish.bitrate ?? 0) }}
                    </span>
                </td>
                <td>
                    <span :title="`${formatBytes(i.stats?.subscribe.bytes ?? 0)} ${i.statsScope === 'clusterNodeWork' ? 'node work' : 'total'}`">
                        {{ formatBitrate(i.stats?.subscribe.bitrate ?? 0) }}
                    </span>
                </td>
                <td>
                    <span>
                        {{ countActiveSessions(i.publish.sessions.filter(t => t.cascade)) + countActiveSessions(i.subscribe.sessions.filter(t => t.cascade)) }}
                    </span>
                </td>
                <td><span>{{ formatTime(i.createdAt) }}</span></td>
                <td>
                    <div class="flex gap-1">
                        <button
                            class="btn btn-sm"
                            :class="{ 'btn-info': previewStreams.includes(i.id) }"
                            @click="handlePreview(i.id)"
                        >Preview</button>
                        <button class="btn btn-sm" @click="handleViewClients(i.id)">Clients</button>
                        <button v-if="showCascade" class="btn btn-sm" @click="handleCascadePushStream(i.id)">Cascade Push</button>
                        <button v-if="features.player" class="btn btn-sm" @click="handleOpenPlayerPage(i.id)">Player</button>
                        <button v-if="features.debugger" class="btn btn-sm" @click="handleOpenDebuggerPage(i.id)">Debugger</button>
                        <button
                            v-if="recordingAvailable"
                            class="btn btn-sm"
                            :class="recordingStates[i.id] ? 'btn-success' : 'btn-info'"
                            @click="openRecordDialog(i.id)"
                        >{{ recordingStates[i.id] ? "Recording" : "Record" }}</button>
                        <slot name="extra-actions" :stream="i" />
                        <!-- disabled buttons don't fire mouse events in
                             some browsers, so the tooltip lives on the
                             wrapper -->
                        <span :title="i.provisioned ? 'Configured streams cannot be deleted' : undefined">
                            <button
                                class="btn btn-sm btn-error"
                                :class="{ 'btn-disabled': i.provisioned }"
                                :disabled="i.provisioned"
                                @click="handleDestroyStream(i.id)"
                            >Destroy</button>
                        </span>
                    </div>
                </td>
            </tr>
            <tr v-if="streamsData.length === 0">
                <td colspan="8" class="text-center">N/A</td>
            </tr>
        </tbody>
    </table>

    <div v-if="recordingAvailable && recordDialogOpen" class="modal modal-open">
        <div class="modal-box">
            <h3 class="font-bold text-lg">Start Recording</h3>
            <p class="py-2 text-sm opacity-80">Stream: <span class="font-mono">{{ recordDialogStreamId }}</span></p>
            <div v-if="recordError" class="alert alert-error my-2">
                <span>{{ recordError }}</span>
            </div>
            <div v-if="recordMpd" class="alert alert-success my-2">
                <span>Recording started. MPD: <span class="font-mono break-all">{{ recordMpd }}</span></span>
            </div>
            <div class="modal-action">
                <template v-if="!recordMpd">
                    <button
                        class="btn btn-primary"
                        :class="{ 'btn-disabled': recordBusy }"
                        :disabled="recordBusy"
                        @click="handleConfirmRecord"
                    >{{ recordBusy ? "Starting…" : "Start" }}</button>
                    <button class="btn btn-ghost" @click="closeRecordDialog">Cancel</button>
                </template>
                <template v-else>
                    <button v-if="features.recordingPlayback" class="btn btn-success" @click="handlePlayNow">Play now</button>
                    <button class="btn btn-ghost" @click="closeRecordDialog">Close</button>
                </template>
            </div>
        </div>
    </div>

    <div v-if="recordingAvailable && confirmStopOpen" class="modal modal-open">
        <div class="modal-box">
            <h3 class="font-bold text-lg">Stop Recording</h3>
            <p class="py-2 text-sm opacity-80">
                Are you sure you want to stop recording for stream
                <span class="font-mono">{{ recordDialogStreamId }}</span>?
            </p>
            <div class="modal-action">
                <button
                    class="btn btn-error"
                    :class="{ 'btn-disabled': confirmStopBusy }"
                    :disabled="confirmStopBusy"
                    @click="handleConfirmStop"
                >{{ confirmStopBusy ? "Stopping…" : "Confirm" }}</button>
                <button class="btn btn-ghost" @click="handleCancelStop">Cancel</button>
            </div>
        </div>
    </div>

    <div class="flex gap-2 p-4">
        <button class="btn btn-sm btn-primary gap-2" @click="handleNewStream">
            <PlusIcon class="size-5 stroke-current" />
            New Stream
        </button>
        <button v-for="s in webStreams" :key="s" class="btn btn-sm" @click="handleOpenWebStream(s)">{{ s }}</button>
    </div>

    <ClientsDialog
        ref="clientsDialog"
        :id="selectedStreamId"
        :sessions="selectedStreamSessions"
        @client-kicked="updateData"
    />

    <template v-if="showCascade">
        <CascadeDialog ref="cascadePullDialog" mode="pull" />
        <CascadeDialog ref="cascadePushDialog" mode="push" />
    </template>

    <PreviewDialog
        v-for="s in previewStreams"
        :key="s"
        :ref="(el) => setPreviewDialog(s, el)"
        :get-whep-url="getWhepUrl"
        @stop="handlePreviewStop(s)"
    />

    <NewStreamDialog
        ref="newStreamDialog"
        @new-stream-id="handleNewStreamId"
        @stream-created="updateData"
    />

    <WebStreamDialog
        v-for="s in webStreams"
        :key="s"
        :ref="(el) => setWebStreamDialog(s, el)"
        :get-whip-url="getWhipUrl"
        @stop="handleWebStreamStop(s)"
    />
</template>
