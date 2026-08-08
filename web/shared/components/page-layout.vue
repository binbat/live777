<script lang="ts">
export interface PageLayoutProps {
    token: string;
    currentView?: string;
    /** Function prop, forwarded to PageHeader (see its docs). */
    onNavigate?: (view: string) => void;
    enabledTools?: {
        debugger?: boolean;
        player?: boolean;
        dash?: boolean;
        recordings?: boolean;
    };
}
</script>

<script setup lang="ts">
import { toRef } from "vue";

import PageHeader from "./page-header.vue";
import { provideToken } from "../context";

const props = defineProps<PageLayoutProps>();

provideToken(toRef(props, "token"));
</script>

<template>
    <PageHeader
        :current-view="currentView"
        :on-navigate="onNavigate"
        :enabled-tools="enabledTools"
    />
    <div class="max-w-screen-xl mx-auto">
        <slot />
    </div>
</template>
