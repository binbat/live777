<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef, watch, watchEffect } from "vue";
import { MediaPlayer, type MediaPlayerClass } from "dashjs";

import { getSegmentUrl } from "../../api";

type BitrateInfo = { index: number; bitrate: number; label: string };

type WindowWithMediaSource = Window & { MediaSource?: typeof MediaSource };
type ManifestStatus = "idle" | "checking" | "waiting" | "ready" | "error";

const MANIFEST_RETRY_MS = 2000;
const MANIFEST_REFRESH_MS = 2000;

function formatTime(sec: number) {
    if (!isFinite(sec)) return "00:00:00";
    const s = Math.max(0, Math.floor(sec));
    const hh = Math.floor(s / 3600);
    const mm = Math.floor((s % 3600) / 60);
    const ss = s % 60;
    const pad = (n: number) => n.toString().padStart(2, "0");
    return `${pad(hh)}:${pad(mm)}:${pad(ss)}`;
}

function manifestHasPlayableSegments(text: string) {
    const doc = new DOMParser().parseFromString(text, "application/xml");
    if (doc.getElementsByTagName("parsererror").length > 0) return false;

    for (const timeline of Array.from(doc.getElementsByTagName("SegmentTimeline"))) {
        if (timeline.getElementsByTagName("S").length > 0) return true;
    }

    if (doc.getElementsByTagName("SegmentURL").length > 0) return true;
    if (doc.getElementsByTagName("SegmentBase").length > 0) return true;

    return Array.from(doc.getElementsByTagName("SegmentTemplate")).some(template =>
        template.hasAttribute("media") && template.hasAttribute("duration")
    );
}

function manifestIsDynamic(text: string) {
    const doc = new DOMParser().parseFromString(text, "application/xml");
    if (doc.getElementsByTagName("parsererror").length > 0) return false;
    return doc.documentElement.getAttribute("type") === "dynamic";
}

const videoEl = useTemplateRef<HTMLVideoElement>("videoEl");
let player: MediaPlayerClass | null = null;
let raf: number | null = null;
let dragging = false;
const progressBarEl = useTemplateRef<HTMLDivElement>("progressBarEl");

const mpd = ref("");
const token = ref("");
const autoplay = ref(true);
const isPlaying = ref(false);
const isMuted = ref(true);
const playbackRate = ref(1);
const duration = ref(0);
const currentTime = ref(0);
const bufferedEnd = ref(0);
const volume = ref(1);
const qualities = ref<BitrateInfo[]>([]);
const qualityIndex = ref<number | "auto">("auto");

const hoverPct = ref<number | null>(null);
const hoverTime = ref(0);
const unsupportedMsg = ref<string | null>(null);
const manifestStatus = ref<ManifestStatus>("idle");
const isLiveManifest = ref(false);

onMounted(() => {
    const params = new URLSearchParams(location.search);
    const mpdParam = params.get("mpd") ?? "";
    const tokenParam = params.get("token") ?? "";
    const auto = params.get("autoplay");
    const mute = params.get("muted");
    mpd.value = mpdParam;
    token.value = tokenParam;
    autoplay.value = auto !== "0";
    isMuted.value = mute !== "0";
});

watch([mpd, token], (_values, _oldValues, onCleanup) => {
    if (!mpd.value) {
        manifestStatus.value = "idle";
        return;
    }

    let disposed = false;
    let timer: number | undefined;

    const checkManifest = async () => {
        manifestStatus.value = manifestStatus.value === "ready" ? "ready" : "checking";
        try {
            const headers: Record<string, string> = {};
            if (token.value) headers["Authorization"] = `Bearer ${token.value}`;
            const res = await fetch(getSegmentUrl(mpd.value), { headers, cache: "no-store" });
            if (!res.ok) throw new Error(`HTTP ${res.status}`);

            const text = await res.text();
            if (!disposed) isLiveManifest.value = manifestIsDynamic(text);
            if (manifestHasPlayableSegments(text)) {
                if (!disposed) manifestStatus.value = "ready";
                return;
            }

            if (!disposed) {
                manifestStatus.value = "waiting";
                timer = window.setTimeout(checkManifest, MANIFEST_RETRY_MS);
            }
        } catch (err) {
            console.warn("[DASH Player] Manifest is not ready:", err);
            if (!disposed) {
                manifestStatus.value = "error";
                timer = window.setTimeout(checkManifest, MANIFEST_RETRY_MS);
            }
        }
    };

    void checkManifest();

    onCleanup(() => {
        disposed = true;
        if (timer !== undefined) window.clearTimeout(timer);
    });
}, { immediate: true });

watch([mpd, isLiveManifest, manifestStatus], (_values, _oldValues, onCleanup) => {
    if (!mpd.value || !isLiveManifest.value || manifestStatus.value !== "ready") return;

    const p = player;
    if (!p) return;

    const interval = window.setInterval(() => {
        try {
            p.refreshManifest(() => { /* ignore refresh callback */ });
        } catch (err) {
            console.warn("[DASH Player] Failed to refresh manifest:", err);
        }
    }, MANIFEST_REFRESH_MS);

    onCleanup(() => window.clearInterval(interval));
}, { immediate: true });

// Initialize dash.js after the manifest has at least one playable segment.
watch([mpd, token, autoplay, manifestStatus], (_values, _oldValues, onCleanup) => {
    if (!videoEl.value || !mpd.value || manifestStatus.value !== "ready") return;

    const p = MediaPlayer().create();
    player = p;

    if (token.value) {
        const key = "Authorization";
        const value = `Bearer ${token.value}`;
        p.extend("RequestModifier", () => ({
            modifyRequestHeader: (xhr: XMLHttpRequest) => {
                xhr.setRequestHeader(key, value);
                return xhr;
            },
            modifyRequestURL: (url: string) => url,
        }), true);
    }

    p.updateSettings({
        streaming: {
            abr: { autoSwitchBitrate: { video: true, audio: true } },
        },
        debug: {
            logLevel: 3,
        },
    });

    p.on(MediaPlayer.events.STREAM_INITIALIZED, () => {
        const v = videoEl.value!;
        duration.value = v.duration || p.duration() || 0;
        const list = p.getRepresentationsByType("video") || [];
        const mapped: BitrateInfo[] = list.map((b, idx) => ({
            index: idx,
            bitrate: b.bandwidth,
            label: `${(b.bandwidth / 1000000).toFixed(2)} Mbps`,
        }));
        qualities.value = mapped;
        qualityIndex.value = "auto";
    });

    // Add error handling to provide better feedback
    p.on(MediaPlayer.events.ERROR, (e: unknown) => {
        console.error("[DASH Player] Error occurred:", e);
        // Don't block playback, just log the error
        // Some errors might be recoverable or non-fatal
    });

    // Log playback started successfully
    p.on(MediaPlayer.events.PLAYBACK_STARTED, () => {
        console.log("[DASH Player] Playback started successfully");
        // Clear unsupported warning if playback actually works
        if (unsupportedMsg.value && unsupportedMsg.value.includes("reports no support")) {
            unsupportedMsg.value = null;
        }
    });

    p.initialize(videoEl.value, getSegmentUrl(mpd.value), autoplay.value);

    onCleanup(() => {
        try { p.reset(); } catch { /* ignore */ }
        player = null;
    });
}, { immediate: true });

// Detect MSE codec/container support before/while initializing
watch([mpd, token, manifestStatus], () => {
    (async () => {
        if (!mpd.value || manifestStatus.value !== "ready") return;
        try {
            const url = getSegmentUrl(mpd.value);
            const headers: Record<string, string> = {};
            if (token.value) headers["Authorization"] = `Bearer ${token.value}`;
            const res = await fetch(url, { headers });
            const txt = await res.text();
            // crude extract first video Representation mimeType & codecs
            const mimeMatch = txt.match(/mimeType\s*=\s*"(video\/(?:mp4|webm))"/i);
            const codecsMatch = txt.match(/codecs\s*=\s*"([^"]+)"/i);
            const mime = mimeMatch?.[1];
            const codecs = codecsMatch?.[1];
            if (!("MediaSource" in window)) {
                unsupportedMsg.value = "This browser does not support MSE. DASH playback is unavailable.";
                return;
            }
            if (mime && codecs) {
                const type = `${mime}; codecs="${codecs}"`;
                const ms = (window as WindowWithMediaSource).MediaSource;
                const ok = !!ms?.isTypeSupported?.(type);
                if (!ok) {
                    // Show warning but still attempt playback
                    unsupportedMsg.value = `Browser reports no support for ${type}. Attempting playback anyway - audio may work, or browser may support it despite reporting otherwise.`;
                    console.warn("[DASH Player] Codec not officially supported, but will attempt playback:", type);
                } else {
                    unsupportedMsg.value = null;
                }
            }
        } catch (err) {
            // ignore detection errors, but log them
            console.warn("[DASH Player] Failed to detect codec support:", err);
        }
    })();
}, { immediate: true });

// Retry autoplay when tab becomes visible (some browsers pause background video to save power)
watch(autoplay, (_value, _oldValue, onCleanup) => {
    const onVisibility = () => {
        if (!autoplay.value) return;
        const v = videoEl.value;
        if (!v) return;
        if (!document.hidden && v.paused) {
            v.play().catch(() => { /* ignore */ });
        }
    };
    document.addEventListener("visibilitychange", onVisibility);
    onCleanup(() => document.removeEventListener("visibilitychange", onVisibility));
}, { immediate: true });

// Sync video element states. flush: "post" so the first run happens after
// mount with the element in place, like the original mount effect — the
// reactive values may already be set from the URL params before then.
watchEffect(
    () => {
        const v = videoEl.value;
        if (!v) return;
        v.muted = isMuted.value;
        v.playbackRate = playbackRate.value;
        v.volume = volume.value;
    },
    { flush: "post" },
);

const onPlay = () => {
    isPlaying.value = true;
};
const onPause = () => {
    isPlaying.value = false;
};
const onLoaded = () => {
    const v = videoEl.value;
    if (!v) return;
    duration.value = v.duration || duration.value;
};
const onProgress = () => {
    const v = videoEl.value;
    if (!v) return;
    try {
        const len = v.buffered.length;
        if (len > 0) bufferedEnd.value = v.buffered.end(len - 1);
    } catch { /* ignore */ }
};
const tick = () => {
    const v = videoEl.value;
    if (!v) return;
    if (!dragging) currentTime.value = v.currentTime;
    raf = requestAnimationFrame(tick);
};

onMounted(() => {
    const v = videoEl.value;
    if (!v) return;
    v.addEventListener("play", onPlay);
    v.addEventListener("pause", onPause);
    v.addEventListener("loadedmetadata", onLoaded);
    v.addEventListener("progress", onProgress);
    raf = requestAnimationFrame(tick);
});

// Controls handlers
const togglePlay = async () => {
    const v = videoEl.value;
    if (!v) return;
    if (v.paused) {
        await v.play().catch(() => { });
    } else {
        v.pause();
    }
};
const seekTo = (t: number) => {
    const v = videoEl.value;
    if (!v) return;
    v.currentTime = Math.max(0, Math.min(duration.value || v.duration || 0, t));
};
const skip = (delta: number) => seekTo(currentTime.value + delta);
const toggleMute = () => {
    isMuted.value = !isMuted.value;
};
const changeRate = (r: number) => {
    playbackRate.value = r;
};
const toggleFullscreen = async () => {
    const el = videoEl.value?.parentElement;
    if (!el) return;
    if (document.fullscreenElement) await document.exitFullscreen();
    else await el.requestFullscreen().catch(() => { });
};
const togglePip = async () => {
    const v = videoEl.value;
    if (!v) return;
    type DocumentPiP = Document & {
        pictureInPictureElement?: Element | null;
        pictureInPictureEnabled?: boolean;
        exitPictureInPicture?: () => Promise<void>;
    };
    type HTMLVideoPiP = HTMLVideoElement & {
        disablePictureInPicture?: boolean;
        requestPictureInPicture?: () => Promise<void>;
    };
    const d = document as DocumentPiP;
    const vv = v as HTMLVideoPiP;
    if (d.pictureInPictureElement) {
        await d.exitPictureInPicture?.().catch(() => { });
    } else if (d.pictureInPictureEnabled && !vv.disablePictureInPicture) {
        await vv.requestPictureInPicture?.().catch(() => { });
    }
};

// Keyboard shortcuts
const onKey = (e: KeyboardEvent) => {
    if ((e.target as HTMLElement)?.tagName === "INPUT") return;
    switch (e.key) {
        case " ": e.preventDefault(); void togglePlay(); break;
        case "ArrowLeft": skip(-5); break;
        case "ArrowRight": skip(5); break;
        case "ArrowUp": volume.value = Math.min(1, volume.value + 0.05); break;
        case "ArrowDown": volume.value = Math.max(0, volume.value - 0.05); break;
        case "m": case "M": toggleMute(); break;
        case "f": case "F": void toggleFullscreen(); break;
    }
};

onMounted(() => {
    window.addEventListener("keydown", onKey);
});

onUnmounted(() => {
    const v = videoEl.value;
    if (v) {
        v.removeEventListener("play", onPlay);
        v.removeEventListener("pause", onPause);
        v.removeEventListener("loadedmetadata", onLoaded);
        v.removeEventListener("progress", onProgress);
    }
    if (raf) cancelAnimationFrame(raf);
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("mousemove", onScrubMouseListener);
});

const pct = computed(() => {
    const d = duration.value || videoEl.value?.duration || 0;
    if (!d) return 0;
    return Math.min(100, Math.max(0, (currentTime.value / d) * 100));
});
const bufPct = computed(() => {
    const d = duration.value || videoEl.value?.duration || 0;
    if (!d) return 0;
    return Math.min(100, Math.max(0, (bufferedEnd.value / d) * 100));
});

// Progress bar interactions
const onScrubMouse = (e: MouseEvent) => {
    // When dragging, the event target is window; fall back to progress bar ref
    const bar: HTMLElement | null = dragging
        ? progressBarEl.value
        : (e.currentTarget as HTMLElement | null);
    if (!bar || typeof bar.getBoundingClientRect !== "function") return;
    const rect = bar.getBoundingClientRect();
    const x = Math.min(Math.max(e.clientX - rect.left, 0), rect.width);
    const pct = x / rect.width;
    hoverPct.value = pct * 100;
    const d = duration.value || videoEl.value?.duration || 0;
    hoverTime.value = pct * d;
    if (dragging) {
        seekTo(pct * d);
    }
};
const onScrubMouseListener: EventListener = (ev) => onScrubMouse(ev as unknown as MouseEvent);
const onScrubUpListener: EventListener = () => onScrubUp();
const onScrubDown = (e: MouseEvent) => {
    dragging = true;
    onScrubMouse(e);
    window.addEventListener("mousemove", onScrubMouseListener);
    window.addEventListener("mouseup", onScrubUpListener, { once: true });
};
const onScrubUp = () => {
    dragging = false;
    window.removeEventListener("mousemove", onScrubMouseListener);
};

const onRateChange = (e: Event) => {
    changeRate(Number((e.target as HTMLSelectElement).value));
};

const onQualityChange = (e: Event) => {
    const val = (e.target as HTMLSelectElement).value;
    if (val === "auto") setAutoQuality(true);
    else setManualQuality(Number(val));
};

const onVolumeInput = (e: Event) => {
    volume.value = Number((e.target as HTMLInputElement).value);
};

const setAutoQuality = (auto: boolean) => {
    const p = player;
    if (!p) return;
    p.updateSettings({ streaming: { abr: { autoSwitchBitrate: { video: auto } } } });
    if (auto) qualityIndex.value = "auto";
};
const setManualQuality = (idx: number) => {
    const p = player;
    if (!p) return;
    setAutoQuality(false);
    try { p.setRepresentationForTypeById("video", idx); } catch { /* ignore */ }
    qualityIndex.value = idx;
};

const RATES = [0.5, 0.75, 1, 1.25, 1.5, 2];
</script>

<template>
    <div id="dash-player">
        <div class="player-shell">
            <div v-if="unsupportedMsg" class="warning">
                {{ unsupportedMsg }}
            </div>
            <video
                ref="videoEl"
                class="player-video"
                :muted="isMuted"
                :autoplay="autoplay"
                playsinline
                @click="togglePlay"
            ></video>
            <div v-if="manifestStatus !== 'ready'" class="status-panel">
                {{
                    !mpd
                        ? "No MPD selected."
                        : manifestStatus === "error"
                            ? "Waiting for the recording manifest..."
                            : "Preparing recording playback..."
                }}
            </div>

            <!-- Controls -->
            <div class="controls">
                <!-- Progress bar -->
                <div
                    ref="progressBarEl"
                    class="progress-bar"
                    @mousedown="onScrubDown"
                    @mousemove="onScrubMouse"
                    @mouseleave="hoverPct = null"
                >
                    <div class="progress-buffer" :style="{ width: `${bufPct}%` }"></div>
                    <div class="progress-played" :style="{ width: `${pct}%` }"></div>
                    <div v-if="hoverPct !== null" class="progress-hover" :style="{ left: `${hoverPct}%` }">
                        <span>{{ formatTime(hoverTime) }}</span>
                    </div>
                </div>

                <div class="toolbar">
                    <div class="left">
                        <button class="btn" :title="isPlaying ? 'Pause (Space)' : 'Play (Space)'" @click="togglePlay">
                            {{ isPlaying ? "❚❚" : "►" }}
                        </button>
                        <button class="btn" title="Back 5s (←)" @click="skip(-5)">-5s</button>
                        <button class="btn" title="Forward 5s (→)" @click="skip(5)">+5s</button>
                        <span class="time">{{ formatTime(currentTime) }} / {{ formatTime(duration || 0) }}</span>
                    </div>
                    <div class="center">
                        <div class="menu">
                            <label>Speed</label>
                            <select :value="playbackRate" @change="onRateChange">
                                <option v-for="r in RATES" :key="r" :value="r">{{ r }}x</option>
                            </select>
                        </div>
                        <div class="menu">
                            <label>Quality</label>
                            <select :value="qualityIndex" @change="onQualityChange">
                                <option value="auto">Auto</option>
                                <option v-for="q in qualities" :key="q.index" :value="q.index">{{ q.label }}</option>
                            </select>
                        </div>
                    </div>
                    <div class="right">
                        <div class="menu volume">
                            <button class="btn" title="Mute (M)" @click="toggleMute">{{ isMuted ? "🔇" : "🔊" }}</button>
                            <input
                                type="range"
                                min="0"
                                max="1"
                                step="0.01"
                                :value="isMuted ? 0 : volume"
                                @input="onVolumeInput"
                            />
                        </div>
                        <button class="btn" title="Picture in Picture" @click="togglePip">🗔</button>
                        <button class="btn" title="Fullscreen (F)" @click="toggleFullscreen">⛶</button>
                    </div>
                </div>
            </div>
        </div>
    </div>
</template>
