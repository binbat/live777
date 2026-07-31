<script setup lang="ts">
import { ref, watch, watchEffect } from "vue";

import * as api from "@/shared/api";
import { useNeedAuthorization } from "@/shared/hooks/use-need-authorization";
import PageLayout from "@/shared/components/page-layout.vue";
import StreamsTable from "@/shared/components/streams-table.vue";
import RecordingsPage from "@/shared/components/recordings-page.vue";

import Login from "./components/login.vue";

const token = ref("");
const [needsAuthorization, setNeedsAuthorization] = useNeedAuthorization(api);
const currentView = ref<"streams" | "recordings">(
    new URLSearchParams(location.search).get("view") === "recordings" ? "recordings" : "streams"
);
const recorderAvailable = ref(false);
const cascadeAvailable = ref(false);

// re-probe optional features whenever the token changes; runs once on
// setup like the original Preact mount effect
watchEffect((onCleanup) => {
    void token.value;
    let disposed = false;
    onCleanup(() => {
        disposed = true;
    });
    (async () => {
        const [recorderStatus, cascadeStatus] = await Promise.all([
            api.probeRecorderFeature(),
            api.probeCascadeFeature(),
        ]);
        if (!disposed) {
            recorderAvailable.value = recorderStatus === "available";
            cascadeAvailable.value = cascadeStatus === "available";
        }
    })();
});

// fall back to the streams view when the recorder is unavailable;
// immediate matches the original Preact effect, which also ran once on
// mount with the initial (unavailable) probe values
watch([currentView, recorderAvailable], () => {
    if (recorderAvailable.value || currentView.value !== "recordings") {
        return;
    }

    const url = new URL(window.location.href);
    url.searchParams.delete("view");
    window.history.replaceState({}, "", url.toString());
    currentView.value = "streams";
}, { immediate: true });

const onLoginSuccess = (t: string) => {
    token.value = t;
    setNeedsAuthorization(false);
};

const navigateToView = (view: string) => {
    const url = new URL(window.location.href);
    if (view === "streams") {
        url.searchParams.delete("view");
    } else {
        url.searchParams.set("view", view);
    }
    window.history.pushState({}, "", url.toString());
    currentView.value = view as "streams" | "recordings";
};

const streamsSSEUrl = api.STREAMS_SSE_URL;
</script>

<template>
    <PageLayout
        :token="token"
        :current-view="currentView"
        :on-navigate="navigateToView"
        :enabled-tools="{ debugger: true, player: true, dash: recorderAvailable, recordings: recorderAvailable }"
    >
        <RecordingsPage v-if="recorderAvailable && currentView === 'recordings'" />
        <StreamsTable
            v-else
            :streams-s-s-e-url="streamsSSEUrl"
            :show-cascade="cascadeAvailable"
            :features="{ debugger: true, player: true, recording: recorderAvailable, autoDetectRecording: false, recordingPlayback: recorderAvailable }"
        />
    </PageLayout>
    <Login :show="needsAuthorization" @success="onLoginSuccess" />
</template>
