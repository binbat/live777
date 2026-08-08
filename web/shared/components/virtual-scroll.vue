<script setup lang="ts" generic="T">
import { computed, ref } from "vue";

interface VirtualScrollProps<T> {
    items: T[];
    itemHeight: number;
    containerHeight: number;
    overscan?: number;
}

const props = withDefaults(defineProps<VirtualScrollProps<T>>(), {
    overscan: 5,
});

const scrollTop = ref(0);

const view = computed(() => {
    const startIndex = Math.floor(scrollTop.value / props.itemHeight);
    const endIndex = Math.min(
        startIndex + Math.ceil(props.containerHeight / props.itemHeight) + props.overscan,
        props.items.length
    );

    const visibleStartIndex = Math.max(0, startIndex - props.overscan);
    const visibleItems = props.items.slice(visibleStartIndex, endIndex);

    return {
        visibleItems: visibleItems.map((item, index) => ({
            item,
            index: visibleStartIndex + index
        })),
        totalHeight: props.items.length * props.itemHeight,
        offsetY: visibleStartIndex * props.itemHeight
    };
});

const handleScroll = (event: Event) => {
    scrollTop.value = (event.target as HTMLDivElement).scrollTop;
};
</script>

<template>
    <div :style="{ height: `${containerHeight}px`, overflow: 'auto' }" @scroll="handleScroll">
        <div :style="{ height: `${view.totalHeight}px`, position: 'relative' }">
            <div :style="{ transform: `translateY(${view.offsetY}px)` }">
                <div v-for="{ item, index } in view.visibleItems" :key="index" :style="{ height: `${itemHeight}px` }">
                    <slot name="item" :item="item" :index="index" />
                </div>
            </div>
        </div>
    </div>
</template>
