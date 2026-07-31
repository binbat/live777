<script lang="ts">
export interface PageHeaderProps {
    currentView?: string;
    /**
     * Kept as a function prop (not an emit) so the component can detect
     * whether navigation is wired up (`onNavigate && tools.recordings`).
     * Parents may bind it with `@navigate` or `:on-navigate` — both land
     * on this prop.
     */
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
import { computed } from "vue";
import ChevronDownIcon from "@/shared/components/icons/chevron-down.vue";
import Monitor from "@/shared/components/icons/monitor.vue";
import Calendar from "@/shared/components/icons/calendar.vue";

import Logo from "/logo.svg";
import { useToken } from "../context";

const props = defineProps<PageHeaderProps>();

const token = useToken();
const tools = computed(() => ({
    debugger: true,
    player: true,
    dash: true,
    recordings: true,
    ...props.enabledTools,
}));

const handleOpenDebuggerPage = () => {
    const params = new URLSearchParams();
    params.set("token", token.value);
    const url = new URL(`/tools/debugger.html?${params.toString()}`, location.origin);
    window.open(url);
};

const handleOpenPlayerPage = () => {
    const params = new URLSearchParams();
    params.set("id", "");
    params.set("autoplay", "");
    params.set("muted", "");
    params.set("reconnect", "3000");
    params.set("token", token.value);
    const url = new URL(`/tools/player.html?${params.toString()}`, location.origin);
    window.open(url);
};

const handleOpenDashPage = () => {
    const url = new URL("/tools/dash.html", location.origin);
    window.open(url);
};

const visibleItems = computed(() => [
    { key: "debugger", label: "Debugger", onClick: handleOpenDebuggerPage, hidden: !tools.value.debugger },
    { key: "player", label: "Player", onClick: handleOpenPlayerPage, hidden: !tools.value.player },
    { key: "dash", label: "DASH Player", onClick: handleOpenDashPage, hidden: !tools.value.dash },
].filter(item => !item.hidden));

const showNavigation = computed(() => !!props.onNavigate && tools.value.recordings);
</script>

<template>
    <div role="navigation" aria-label="Navbar" class="navbar bg-base-300 px-0">
        <div class="flex grow max-w-screen-xl px-4 mx-auto">
            <div class="flex gap-2 mr-auto group">
                <img
                    :src="Logo"
                    class="h-8 transition-[filter] duration-200 ease-in-out group-hover:drop-shadow-[0_0_1em_#1991e8aa]"
                />
                <span class="text-xl font-bold">Live777</span>
            </div>

            <!-- Navigation Tabs -->
            <div v-if="showNavigation" class="flex-1 flex justify-center">
                <div role="tablist" class="tabs tabs-boxed tabs-sm">
                    <a
                        role="tab"
                        class="tab"
                        :class="{ 'tab-active': currentView === 'streams' }"
                        @click="onNavigate?.('streams')"
                    >
                        <Monitor class="w-4 h-4 mr-2" />
                        Streams
                    </a>
                    <a
                        v-if="tools.recordings"
                        role="tab"
                        class="tab"
                        :class="{ 'tab-active': currentView === 'recordings' }"
                        @click="onNavigate?.('recordings')"
                    >
                        <Calendar class="w-4 h-4 mr-2" />
                        Recordings
                    </a>
                </div>
            </div>

            <div v-if="visibleItems.length > 0" role="listbox" class="dropdown dropdown-end">
                <label tabindex="1" class="btn btn-ghost gap-2">
                    Tools
                    <ChevronDownIcon class="size-4 stroke-current" />
                </label>
                <ul tabindex="0" role="menu" class="dropdown-content menu p-2 shadow rounded-box bg-base-300 mt-4 z-10">
                    <li v-for="item in visibleItems" :key="item.key" role="menuitem">
                        <a @click="item.onClick">{{ item.label }}</a>
                    </li>
                </ul>
            </div>
        </div>
    </div>
</template>
