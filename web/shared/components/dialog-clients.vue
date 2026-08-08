<script lang="ts">
export interface IClientsDialog {
    show(): void;
}
</script>

<script setup lang="ts">
import { useTemplateRef } from "vue";

import { type Session, deleteSession } from "../api";
import { formatBitrate, formatTime } from "../utils";

interface Props {
    id: string;
    sessions: Session[];
}

const props = defineProps<Props>();

const emit = defineEmits<{
    "client-kicked": [];
}>();

const dialogEl = useTemplateRef<HTMLDialogElement>("dialogEl");

const show = () => {
    dialogEl.value?.showModal();
};

defineExpose({ show });

const handleKickClient = async (streamId: string, clientId: string) => {
    await deleteSession(streamId, clientId);
    emit("client-kicked");
};
</script>

<template>
    <dialog ref="dialogEl" aria-label="Modal" aria-hidden="true" class="modal">
        <div class="modal-box min-w-md max-w-[unset] w-[unset]">
            <div class="w-full text-xl mb-2">
                <h3 class="font-bold">Clients of {{ id }}</h3>
            </div>
            <div>
                <table class="table">
                    <thead>
                        <tr>
                            <th><span>ID</span></th>
                            <td><span>State</span></td>
                            <td><span>Out</span></td>
                            <td><span>Creation Time</span></td>
                            <td><span>Leave Time</span></td>
                            <td><span>Operation</span></td>
                        </tr>
                    </thead>
                    <tbody>
                        <tr v-for="c in sessions" :key="c.id">
                            <th><span>{{ c.id + (c.reforward ? "(reforward)" : "") }}</span></th>
                            <td><span>{{ c.state }}</span></td>
                            <td><span>{{ formatBitrate(c.stats?.bitrate ?? 0) }}</span></td>
                            <td><span>{{ formatTime(c.createdAt) }}</span></td>
                            <td><span>{{ c.leaveAt ? formatTime(c.leaveAt) : "-" }}</span></td>
                            <td>
                                <button
                                    class="btn btn-sm btn-error"
                                    :class="{ 'btn-disabled': c.state === 'closed' }"
                                    :disabled="c.state === 'closed'"
                                    @click="handleKickClient(id, c.id)"
                                >Kick</button>
                            </td>
                        </tr>
                        <tr v-if="sessions.length === 0">
                            <td colspan="6" class="text-center">N/A</td>
                        </tr>
                    </tbody>
                </table>
            </div>
            <div class="modal-action">
                <form method="dialog">
                    <button class="btn">Close</button>
                </form>
            </div>
        </div>
    </dialog>
</template>
