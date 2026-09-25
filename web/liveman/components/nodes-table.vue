<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from "vue";
import { ArrowPathIcon, EllipsisHorizontalIcon } from "@heroicons/vue/24/outline";

import { useToken } from "@/shared/context";
import { useRefreshTimer } from "@/shared/hooks/use-refresh-timer";

import { type Node, getNodes } from "../api";

async function getNodesSorted(): Promise<Node[]> {
    try {
        const nodes = await getNodes();
        return nodes.sort((a, b) => a.alias.localeCompare(b.alias));
    } catch {
        return [];
    }
}

const { data: nodes, isRefreshing, updateData, toggleTimer } = useRefreshTimer<Node[]>([], getNodesSorted);
const token = useToken();

// refresh when the token changes; immediate matches the original Preact
// effect, which also ran once on mount (initial fetch)
watch(token, () => {
    void updateData();
}, { immediate: true });

function strategyEntries(strategy: Node["strategy"]): [string, string | number | boolean][] {
    return Object.entries(strategy ?? {});
}

function nodeUrl(alias: string): string {
    const urlObject = new URL(location.href);
    urlObject.searchParams.set("nodes", alias);
    return urlObject.toString();
}

// The strategy popover teleports to <body>: the table scrolls
// horizontally inside an overflow-x-auto wrapper on mobile, which
// would clip an in-cell dropdown (overflow-x: auto forces overflow-y
// clipping too).
const strategyMenu = ref<{ alias: string; top: number; left: number; openedAt: number } | null>(null);
const toggleStrategyMenu = (alias: string, event: MouseEvent) => {
    if (strategyMenu.value?.alias === alias) {
        strategyMenu.value = null;
        return;
    }
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    strategyMenu.value = { alias, top: rect.bottom + 4, left: rect.left, openedAt: Date.now() };
};
const closeStrategyMenu = () => {
    strategyMenu.value = null;
};
// scroll events from a scroll-into-view (e.g. touch tap) can arrive
// right after the click that opened the menu; ignore those
const passiveCloseStrategyMenu = () => {
    if (strategyMenu.value && Date.now() - strategyMenu.value.openedAt < 300) return;
    strategyMenu.value = null;
};
onMounted(() => window.addEventListener("resize", passiveCloseStrategyMenu));
onUnmounted(() => window.removeEventListener("resize", passiveCloseStrategyMenu));
</script>

<template>
    <div class="flex flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2">
        <span class="font-bold text-lg">Nodes</span>
        <div aria-label="Badge" class="badge badge-ghost font-bold mr-auto">{{ nodes.length }}</div>
        <button class="btn btn-sm btn-ghost gap-2" @click="toggleTimer">
            Auto Refresh
            <input type="checkbox" class="checkbox checkbox-xs" :checked="isRefreshing" />
        </button>
        <button class="btn btn-sm btn-ghost gap-2" @click="updateData">
            Refresh
            <ArrowPathIcon class="size-4 stroke-current" />
        </button>
    </div>

    <div class="overflow-x-auto" @scroll.passive="passiveCloseStrategyMenu">
        <table class="table">
            <thead>
                <tr>
                    <th><span>Alias</span></th>
                    <td><span>Status</span></td>
                    <td><span>Delay</span></td>
                    <td><span>Strategy</span></td>
                    <td><span>API URL</span></td>
                </tr>
            </thead>
            <tbody>
                <tr v-for="n in nodes" :key="n.alias">
                    <th><span>{{ n.alias }}</span></th>
                    <td><span>{{ n.status }}</span></td>
                    <td><span>{{ n.duration }}</span></td>
                    <td>
                        <span v-if="strategyEntries(n.strategy).length <= 1" class="font-mono">
                            {{ strategyEntries(n.strategy)[0]?.join(" = ") ?? "-" }}
                        </span>
                        <button
                            v-else
                            class="font-mono flex items-center gap-1"
                            @click="toggleStrategyMenu(n.alias, $event)"
                        >
                            <span>{{ strategyEntries(n.strategy)[0].join(" = ") }}</span>
                            <EllipsisHorizontalIcon class="size-4" />
                        </button>
                    </td>
                    <td>
                        <a class="link link-hover break-all" :href="nodeUrl(n.alias)" target="_blank">{{ nodeUrl(n.alias) }}</a>
                    </td>
                </tr>
                <tr v-if="nodes.length === 0">
                    <td colspan="5" class="text-center">N/A</td>
                </tr>
            </tbody>
        </table>
    </div>

    <Teleport to="body">
        <template v-if="strategyMenu">
            <div class="fixed inset-0 z-40" @click="closeStrategyMenu" />
            <div
                class="fixed z-50 menu p-2 shadow bg-base-100 rounded-box"
                :style="{ top: `${strategyMenu.top}px`, left: `${strategyMenu.left}px` }"
            >
                <table class="table table-xs">
                    <tbody>
                        <tr
                            v-for="[k, v] in strategyEntries(nodes.find(n => n.alias === strategyMenu?.alias)?.strategy)"
                            :key="k"
                        >
                            <th><span class="text-sm font-mono">{{ k }}</span></th>
                            <td><span class="text-sm font-mono">{{ v }}</span></td>
                        </tr>
                    </tbody>
                </table>
            </div>
        </template>
    </Teleport>
</template>
