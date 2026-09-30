<script lang="ts">
export interface INodeInfoDialog {
    show(node: Node): void;
}
</script>

<script setup lang="ts">
import { ref, useTemplateRef } from "vue";

import type { Node } from "../api";

const node = ref<Node | null>(null);
const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");

const show = (n: Node) => {
    node.value = n;
    dialogEl.value?.showModal();
};

defineExpose({ show });
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="true" class="modal">
        <div class="modal-box max-w-md">
            <div class="w-full text-xl mb-2">
                <h3 class="font-bold">Node {{ node?.alias }}</h3>
            </div>
            <table class="table table-xs">
                <tbody>
                    <tr>
                        <th><span>Status</span></th>
                        <td><span class="font-mono">{{ node?.status }}</span></td>
                    </tr>
                    <tr>
                        <th><span>Delay</span></th>
                        <td><span class="font-mono tabular-nums">{{ node?.duration }}</span></td>
                    </tr>
                    <tr>
                        <th><span>API URL</span></th>
                        <td><span class="font-mono break-all">{{ node?.url }}</span></td>
                    </tr>
                    <tr>
                        <th><span>Version</span></th>
                        <td><span class="font-mono">{{ node?.info?.version ?? "-" }}</span></td>
                    </tr>
                    <tr>
                        <th><span>Git commit</span></th>
                        <td><span class="font-mono">{{ node?.info?.gitHash ?? "-" }}</span></td>
                    </tr>
                    <tr>
                        <th><span>Build time</span></th>
                        <td><span class="font-mono">{{ node?.info?.buildTime ?? "-" }}</span></td>
                    </tr>
                    <tr>
                        <th><span>Features</span></th>
                        <td>
                            <template v-if="node?.info?.features?.length">
                                <span
                                    v-for="f in node.info.features"
                                    :key="f"
                                    class="badge badge-ghost badge-sm font-mono mr-1"
                                >{{ f }}</span>
                            </template>
                            <span v-else class="font-mono">-</span>
                        </td>
                    </tr>
                </tbody>
            </table>
            <div class="modal-action">
                <form method="dialog">
                    <button class="btn">Close</button>
                </form>
            </div>
        </div>
    </dialog>
</template>
