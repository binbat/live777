import { computed, onUnmounted, ref, shallowRef, type ComputedRef, type ShallowRef } from "vue";

export interface UseRefreshTimerReturn<T> {
    data: ShallowRef<T>;
    isRefreshing: ComputedRef<boolean>;
    updateData: () => Promise<void>;
    toggleTimer: () => void;
}

export function useRefreshTimer<T>(initial: T, fetchData: () => Promise<T>, timeout = 3000): UseRefreshTimerReturn<T> {
    const data = shallowRef(initial) as ShallowRef<T>;
    const refreshTimer = ref(-1);
    const isRefreshing = computed(() => refreshTimer.value > 0);

    const updateData = async () => {
        data.value = await fetchData();
    };

    const stopTimer = () => {
        if (refreshTimer.value > 0) {
            window.clearInterval(refreshTimer.value);
            refreshTimer.value = -1;
        }
    };

    const toggleTimer = () => {
        if (isRefreshing.value) {
            stopTimer();
        } else {
            void updateData();
            refreshTimer.value = window.setInterval(updateData, timeout);
        }
    };

    onUnmounted(stopTimer);

    return {
        data,
        isRefreshing,
        updateData,
        toggleTimer
    };
}
