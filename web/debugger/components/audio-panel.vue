<script setup lang="ts">
import { AudioMonitor } from "player-core";
import {
    computed,
    onMounted,
    onUnmounted,
    ref,
    useTemplateRef,
    watch,
} from "vue";

const SpectrumBars = 48;
const CanvasWidth = 280;
const CanvasHeight = 64;

const props = defineProps<{
    stream: MediaStream;
}>();

type AudioTrackState = {
    id: string;
    readyState: MediaStreamTrackState;
    muted: boolean;
};

const canvasRef = useTemplateRef<HTMLCanvasElement>("canvasRef");
const levelPercent = ref(0);
const suspended = ref(false);
const trackStates = ref<AudioTrackState[]>([]);

let monitor: AudioMonitor | null = null;
let animationFrame = 0;
const trackListeners = new Map<
    string,
    { track: MediaStreamTrack; handler: () => void }
>();

const syncTrackStates = () => {
    trackStates.value = props.stream.getAudioTracks().map((track) => ({
        id: track.id,
        readyState: track.readyState,
        muted: track.muted,
    }));
};

const attachTrackListeners = () => {
    for (const track of props.stream.getAudioTracks()) {
        if (trackListeners.has(track.id)) continue;
        const handler = () => syncTrackStates();
        track.addEventListener("mute", handler);
        track.addEventListener("unmute", handler);
        track.addEventListener("ended", handler);
        trackListeners.set(track.id, { track, handler });
    }
};

const detachTrackListeners = () => {
    for (const { track, handler } of trackListeners.values()) {
        track.removeEventListener("mute", handler);
        track.removeEventListener("unmute", handler);
        track.removeEventListener("ended", handler);
    }
    trackListeners.clear();
};

const stopMonitor = () => {
    monitor?.close();
    monitor = null;
    levelPercent.value = 0;
};

// MediaStream.addTrack() does not fire "addtrack" (only platform-added
// tracks do), so track arrival is detected by polling in the rAF loop.
let knownTrackIds = "";

const syncTracksIfChanged = () => {
    const ids = props.stream
        .getAudioTracks()
        .map((track) => track.id)
        .join(",");
    if (ids === knownTrackIds) return;
    knownTrackIds = ids;
    syncTrackStates();
    attachTrackListeners();
};

const drawSpectrum = () => {
    const canvas = canvasRef.value;
    if (!canvas || !monitor) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const spectrum = monitor.spectrum();
    // Speech energy concentrates in the lower bins; skip the top quarter.
    const usableBins = Math.floor(spectrum.length * 0.75);
    ctx.clearRect(0, 0, CanvasWidth, CanvasHeight);
    const barWidth = CanvasWidth / SpectrumBars;
    for (let i = 0; i < SpectrumBars; i++) {
        const bin = Math.floor((i / SpectrumBars) * usableBins);
        const value = spectrum[bin] / 255;
        const height = Math.max(1, value * CanvasHeight);
        ctx.fillStyle = `rgba(59, 130, 246, ${0.3 + value * 0.7})`;
        ctx.fillRect(i * barWidth, CanvasHeight - height, barWidth - 1, height);
    }
};

const tick = () => {
    syncTracksIfChanged();
    if (!monitor && props.stream.getAudioTracks().length > 0) {
        // The audio track can arrive after this panel is already mounted.
        monitor = new AudioMonitor(props.stream);
        void monitor.resume();
        suspended.value = monitor.state === "suspended";
    }
    if (monitor) {
        // Throttle reactive updates: only re-render when the rounded
        // percent changes; the canvas draws every frame regardless.
        const percent = Math.round(monitor.level() * 100);
        if (percent !== levelPercent.value) {
            levelPercent.value = percent;
        }
        drawSpectrum();
    }
    animationFrame = requestAnimationFrame(tick);
};

const resumeFromGesture = () => {
    if (!monitor) return;
    void monitor.resume().then(() => {
        suspended.value = monitor?.state === "suspended";
    });
};

const levelColor = computed(() => {
    if (levelPercent.value < 2) return "#9ca3af";
    if (levelPercent.value < 50) return "#16a34a";
    if (levelPercent.value < 85) return "#d97706";
    return "#dc2626";
});

watch(
    () => props.stream,
    (_stream, _previous, onCleanup) => {
        knownTrackIds = "";
        syncTracksIfChanged();
        onCleanup(() => {
            detachTrackListeners();
            stopMonitor();
        });
    },
    { immediate: true },
);

onMounted(() => {
    const canvas = canvasRef.value;
    if (canvas) {
        const dpr = window.devicePixelRatio || 1;
        canvas.width = CanvasWidth * dpr;
        canvas.height = CanvasHeight * dpr;
        canvas.getContext("2d")?.scale(dpr, dpr);
    }
    animationFrame = requestAnimationFrame(tick);
});

onUnmounted(() => {
    cancelAnimationFrame(animationFrame);
});
</script>

<template>
    <section>
        <h5>Audio Monitor</h5>
        <div v-if="trackStates.length === 0">No audio track</div>
        <template v-else>
            <div v-for="t in trackStates" :key="t.id">
                Track: {{ t.readyState
                }}<template v-if="t.readyState === 'live'">
                    ({{ t.muted ? "no RTP flow" : "receiving" }}) </template
                ><template v-if="suspended">
                    —
                    <button type="button" @click="resumeFromGesture">
                        resume analysis
                    </button>
                </template>
            </div>
            <div
                style="display: flex; align-items: center; gap: 0.5rem; margin: 0.25rem 0"
            >
                <div
                    style="flex: 1; height: 0.75rem; background: #e5e7eb; border-radius: 0.25rem; overflow: hidden"
                >
                    <div
                        :style="{
                            width: `${levelPercent}%`,
                            height: '100%',
                            background: levelColor,
                            transition: 'width 80ms linear',
                        }"
                    />
                </div>
                <span style="min-width: 4ch; text-align: right"
                    >{{ levelPercent }}%</span
                >
            </div>
            <canvas
                ref="canvasRef"
                :style="{
                    width: `${CanvasWidth}px`,
                    height: `${CanvasHeight}px`,
                    display: 'block',
                    margin: '0.25rem auto',
                    background: '#f8fafc',
                    borderRadius: '0.25rem',
                }"
            />
        </template>
    </section>
</template>
