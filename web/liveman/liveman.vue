<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef, watch } from "vue";

import { useNeedAuthorization } from "@/shared/hooks/use-need-authorization";
import StreamsTable from "@/shared/components/streams-table.vue";
import PageLayout from "@/shared/components/page-layout.vue";
import RecordingsPage from "@/shared/components/recordings-page.vue";
import * as sharedApi from "@/shared/api";

import * as livemanApi from "./api";
import Login from "./components/login.vue";
import NodesTable from "./components/nodes-table.vue";
import StreamTokenDialog, { type IStreamTokenDialog } from "./components/dialog-token.vue";

const TOKEN_KEY = "liveman_auth_token";
const savedToken = localStorage.getItem(TOKEN_KEY) ?? "";
const savedTokenValue = savedToken.split(" ")[1] ?? "";
if (savedToken) {
    livemanApi.setAuthToken(savedToken);
    sharedApi.setAuthToken(savedToken);
}

const initialNodes = new URLSearchParams(location.search).getAll("nodes");

const token = ref(savedTokenValue);
const [needsAuthorization, setNeedsAuthorization] = useNeedAuthorization(livemanApi);
const onLoginSuccess = (tokenType: string, tokenValue: string) => {
    token.value = tokenValue;
    setNeedsAuthorization(false);
    localStorage.setItem(TOKEN_KEY, `${tokenType} ${tokenValue}`);
};

// View state management
const currentView = ref<"streams" | "recordings">("streams");

// Initialize view from URL params
const handlePopState = () => {
    const newParams = new URLSearchParams(location.search);
    const newView = (newParams.get("view") as "streams" | "recordings") || "streams";

    currentView.value = newView;
};

onMounted(() => {
    const params = new URLSearchParams(location.search);
    const view = params.get("view") as "streams" | "recordings";

    if (view) {
        currentView.value = view;
    }

    window.addEventListener("popstate", handlePopState);
});
onUnmounted(() => {
    window.removeEventListener("popstate", handlePopState);
});

const navigateToView = (view: string) => {
    const url = new URL(window.location.href);
    url.searchParams.set("view", view);
    window.history.pushState({}, "", url.toString());
    currentView.value = view as "streams" | "recordings";
};

const filterNodes = ref<string[]>(initialNodes);
// the Preact original re-synced from location.search on every re-render
// where the query string had changed; navigations and popstate both go
// through currentView, so watching it covers the same cases
watch(currentView, () => {
    const params = new URLSearchParams(location.search);
    filterNodes.value = params.getAll("nodes");
});

// SSE pushes of the merged cluster view, same as liveion's dashboard; the
// nodes filter rides along as a query param (reactive, so the table
// reconnects when it changes).
const streamsSSEUrl = computed(() => {
    if (filterNodes.value.length === 0) {
        return sharedApi.STREAMS_SSE_URL;
    }
    const params = new URLSearchParams();
    filterNodes.value.forEach(v => params.append("nodes", v));
    return `${sharedApi.STREAMS_SSE_URL}?${params.toString()}`;
});

const getWhxpUrl = (whxp: "whep" | "whip", streamId: string) => {
    let url = `/${whxp}/${streamId}`;
    if (filterNodes.value.length > 0) {
        const params = new URLSearchParams();
        filterNodes.value.forEach(v => params.append("nodes", v));
        url += `?${params.toString()}`;
    }
    return new URL(url, location.origin).toString();
};

const getWhepUrl = (streamId: string) => getWhxpUrl("whep", streamId);
const getWhipUrl = (streamId: string) => getWhxpUrl("whip", streamId);

// Aliases of the nodes currently hosting a stream, for the source
// dialog's cluster mode (the encoder source lives on exactly one of them).
const getStreamNodes = async (streamId: string) => {
    const detail = await livemanApi.getStreamDetail(streamId);
    return Object.keys(detail);
};

const streamTokenDialog = useTemplateRef<IStreamTokenDialog>("streamTokenDialog");
</script>

<template>
    <PageLayout
        :token="token"
        :current-view="currentView"
        :on-navigate="navigateToView"
    >
        <RecordingsPage v-if="currentView === 'recordings'" />
        <template v-else>
            <NodesTable v-if="filterNodes.length === 0" />
            <StreamsTable
                :streams-s-s-e-url="streamsSSEUrl"
                :get-whep-url="getWhepUrl"
                :get-whip-url="getWhipUrl"
                :get-stream-nodes="getStreamNodes"
                :features="{ autoDetectRecording: true, source: true }"
            >
                <template #extra-actions="{ stream }">
                    <a @click="streamTokenDialog?.show(stream.id)">Create token</a>
                </template>
            </StreamsTable>
        </template>
    </PageLayout>
    <StreamTokenDialog ref="streamTokenDialog" />
    <Login :show="needsAuthorization" @success="onLoginSuccess" />
</template>
