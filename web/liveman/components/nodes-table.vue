<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ArrowPathIcon, ChevronDownIcon, ChevronUpIcon } from "@heroicons/vue/24/outline";

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

function nodeUrl(alias: string): string {
    const urlObject = new URL(location.href);
    urlObject.searchParams.set("nodes", alias);
    return urlObject.toString();
}

function delayMs(n: Node): number {
    const m = /^(\d+)ms$/.exec(n.duration);
    return m ? parseInt(m[1]) : Number.POSITIVE_INFINITY;
}

type SortKey = "alias" | "status" | "delay";

const SORT_COLUMNS: { key: SortKey; label: string }[] = [
    { key: "alias", label: "Alias" },
    { key: "status", label: "Status" },
    { key: "delay", label: "Delay" },
];

const SORT_FNS: Record<SortKey, (a: Node, b: Node) => number> = {
    alias: (a, b) => a.alias.localeCompare(b.alias),
    status: (a, b) => a.status.localeCompare(b.status),
    delay: (a, b) => delayMs(a) - delayMs(b),
};

// Component-side sorting keeps row order stable across auto-refresh
// (the server's map order is not meaningful); alias is the tie-break.
const sortKey = ref<SortKey>("alias");
const sortAsc = ref(true);

const sortedNodes = computed(() => {
    const cmp = SORT_FNS[sortKey.value];
    const dir = sortAsc.value ? 1 : -1;
    return [...nodes.value].sort((a, b) => {
        const primary = cmp(a, b);
        return primary !== 0 ? dir * primary : a.alias.localeCompare(b.alias);
    });
});

const toggleSort = (key: SortKey) => {
    if (sortKey.value === key) {
        sortAsc.value = !sortAsc.value;
    } else {
        sortKey.value = key;
        sortAsc.value = true;
    }
};
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

    <div class="overflow-x-auto">
        <table class="table">
            <thead>
                <tr>
                    <th v-for="col in SORT_COLUMNS" :key="col.key">
                        <button
                            class="inline-flex cursor-pointer select-none items-center gap-0.5"
                            @click="toggleSort(col.key)"
                        >
                            {{ col.label }}
                            <ChevronUpIcon v-if="sortKey === col.key && sortAsc" class="size-3.5" />
                            <ChevronDownIcon v-else-if="sortKey === col.key" class="size-3.5" />
                        </button>
                    </th>
                    <td><span>API URL</span></td>
                </tr>
            </thead>
            <tbody>
                <tr v-for="n in sortedNodes" :key="n.alias">
                    <th><span>{{ n.alias }}</span></th>
                    <td><span>{{ n.status }}</span></td>
                    <td><span class="tabular-nums">{{ n.duration }}</span></td>
                    <td>
                        <a class="link link-hover break-all" :href="nodeUrl(n.alias)" target="_blank">{{ nodeUrl(n.alias) }}</a>
                    </td>
                </tr>
                <tr v-if="sortedNodes.length === 0">
                    <td colspan="4" class="text-center">N/A</td>
                </tr>
            </tbody>
        </table>
    </div>
</template>
