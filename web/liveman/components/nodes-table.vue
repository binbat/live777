<script setup lang="ts">
import { watch } from "vue";
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
</script>

<template>
    <div class="flex items-center gap-2 px-4 h-12">
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

    <table class="table overflow-x-auto">
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
                    <div v-else class="dropdown dropdown-hover">
                        <label tabindex="0" class="font-mono flex items-center gap-1">
                            <span>{{ strategyEntries(n.strategy)[0].join(" = ") }}</span>
                            <EllipsisHorizontalIcon class="size-4" />
                        </label>
                        <ul tabindex="0" role="menu" class="dropdown-content menu p-2 shadow bg-base-100 rounded-box z-10 mx-[-1rem]">
                            <table class="table table-xs">
                                <tbody>
                                    <tr v-for="[k, v] in strategyEntries(n.strategy)" :key="k">
                                        <th><span class="text-sm font-mono">{{ k }}</span></th>
                                        <td><span class="text-sm font-mono">{{ v }}</span></td>
                                    </tr>
                                </tbody>
                            </table>
                        </ul>
                    </div>
                </td>
                <td>
                    <a class="link link-hover" :href="nodeUrl(n.alias)" target="_blank">{{ nodeUrl(n.alias) }}</a>
                </td>
            </tr>
            <tr v-if="nodes.length === 0">
                <td colspan="5" class="text-center">N/A</td>
            </tr>
        </tbody>
    </table>
</template>
