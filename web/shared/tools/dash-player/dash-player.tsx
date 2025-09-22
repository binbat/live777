import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import dashjs from 'dashjs';

import { getSegmentUrl } from '@/liveman/api';

type BitrateInfo = { index: number; bitrate: number; label: string };

type WindowWithMediaSource = Window & { MediaSource?: typeof MediaSource };

function formatTime(sec: number) {
    if (!isFinite(sec)) return '00:00:00';
    const s = Math.max(0, Math.floor(sec));
    const hh = Math.floor(s / 3600);
    const mm = Math.floor((s % 3600) / 60);
    const ss = s % 60;
    const pad = (n: number) => n.toString().padStart(2, '0');
    return `${pad(hh)}:${pad(mm)}:${pad(ss)}`;
}

export function DashPlayer() {
    const refVideo = useRef<HTMLVideoElement>(null);
    const refPlayer = useRef<dashjs.MediaPlayerClass | null>(null);
    const refRaf = useRef<number | null>(null);
    const refDragging = useRef(false);
    const refProgressBar = useRef<HTMLDivElement>(null);

    const [mpd, setMpd] = useState('');
    const [token, setToken] = useState('');
    const [autoplay, setAutoplay] = useState(true);
    const [isPlaying, setIsPlaying] = useState(false);
    const [isMuted, setIsMuted] = useState(true);
    const [playbackRate, setPlaybackRate] = useState(1);
    const [duration, setDuration] = useState(0);
    const [currentTime, setCurrentTime] = useState(0);
    const [bufferedEnd, setBufferedEnd] = useState(0);
    const [volume, setVolume] = useState(1);
    const [qualities, setQualities] = useState<BitrateInfo[]>([]);
    const [qualityIndex, setQualityIndex] = useState<number | 'auto'>('auto');

    const [hoverPct, setHoverPct] = useState<number | null>(null);
    const [hoverTime, setHoverTime] = useState(0);
    const [unsupportedMsg, setUnsupportedMsg] = useState<string | null>(null);

    useEffect(() => {
        const params = new URLSearchParams(location.search);
        const mpdParam = params.get('mpd') ?? '';
        const tokenParam = params.get('token') ?? '';
        const auto = params.get('autoplay');
        const mute = params.get('muted');
        setMpd(mpdParam);
        setToken(tokenParam);
        setAutoplay(auto !== '0');
        setIsMuted(mute !== '0');
    }, []);

    // Initialize dash.js
    useEffect(() => {
        if (!refVideo.current || !mpd) return;

        const player = dashjs.MediaPlayer().create();
        refPlayer.current = player;

        if (token) {
            const key = 'Authorization';
            const value = `Bearer ${token}`;
            player.extend('RequestModifier', () => ({
                modifyRequestHeader: (xhr: XMLHttpRequest) => {
                    xhr.setRequestHeader(key, value);
                    return xhr;
                },
                modifyRequestURL: (url: string) => url,
            }), true);
        }

        player.updateSettings({
            streaming: {
                abr: { autoSwitchBitrate: { video: true, audio: true } },
            },
            debug: {
                logLevel: 3,
            },
        });

        player.on(dashjs.MediaPlayer.events.STREAM_INITIALIZED, () => {
            const v = refVideo.current!;
            setDuration(v.duration || player.duration() || 0);
            const list = player.getBitrateInfoListFor('video') || [];
            const mapped: BitrateInfo[] = list.map((b, idx) => ({
                index: idx,
                bitrate: b.bitrate,
                label: `${(b.bitrate / 1000000).toFixed(2)} Mbps`,
            }));
            setQualities(mapped);
            setQualityIndex('auto');
        });

        player.initialize(refVideo.current, getSegmentUrl(mpd), autoplay);

        return () => {
            try { player.reset(); } catch { /* ignore */ }
            refPlayer.current = null;
        };
    }, [mpd, token, autoplay]);

    // Detect MSE codec/container support before/while initializing
    useEffect(() => {
        (async () => {
            if (!mpd) return;
            try {
                const url = getSegmentUrl(mpd);
                const headers: Record<string, string> = {};
                if (token) headers['Authorization'] = `Bearer ${token}`;
                const res = await fetch(url, { headers });
                const txt = await res.text();
                // crude extract first video Representation mimeType & codecs
                const mimeMatch = txt.match(/mimeType\s*=\s*"(video\/(?:mp4|webm))"/i);
                const codecsMatch = txt.match(/codecs\s*=\s*"([^"]+)"/i);
                const mime = mimeMatch?.[1];
                const codecs = codecsMatch?.[1];
                if (!('MediaSource' in window)) {
                    setUnsupportedMsg('This browser does not support MSE. DASH playback is unavailable.');
                    return;
                }
                if (mime && codecs) {
                    const type = `${mime}; codecs="${codecs}"`;
                    const ms = (window as WindowWithMediaSource).MediaSource;
                    const ok = !!ms?.isTypeSupported?.(type);
                    if (!ok) {
                        setUnsupportedMsg(`Browser does not support ${type}. Video may not play (audio only).`);
                    } else {
                        setUnsupportedMsg(null);
                    }
                }
            } catch {
                // ignore detection errors
            }
        })();
    }, [mpd, token]);

    // Retry autoplay when tab becomes visible (some browsers pause background video to save power)
    useEffect(() => {
        const onVisibility = () => {
            if (!autoplay) return;
            const v = refVideo.current;
            if (!v) return;
            if (!document.hidden && v.paused) {
                v.play().catch(() => { /* ignore */ });
            }
        };
        document.addEventListener('visibilitychange', onVisibility);
        return () => document.removeEventListener('visibilitychange', onVisibility);
    }, [autoplay]);

    // Sync video element states
    useEffect(() => {
        const v = refVideo.current;
        if (!v) return;
        v.muted = isMuted;
        v.playbackRate = playbackRate;
        v.volume = volume;
    }, [isMuted, playbackRate, volume]);

    useEffect(() => {
        const v = refVideo.current;
        if (!v) return;
        const onPlay = () => setIsPlaying(true);
        const onPause = () => setIsPlaying(false);
        const onLoaded = () => setDuration(v.duration || duration);
        const onProgress = () => {
            try {
                const len = v.buffered.length;
                if (len > 0) setBufferedEnd(v.buffered.end(len - 1));
            } catch { /* ignore */ }
        };
        const tick = () => {
            if (!refDragging.current) setCurrentTime(v.currentTime);
            refRaf.current = requestAnimationFrame(tick);
        };
        v.addEventListener('play', onPlay);
        v.addEventListener('pause', onPause);
        v.addEventListener('loadedmetadata', onLoaded);
        v.addEventListener('progress', onProgress);
        refRaf.current = requestAnimationFrame(tick);
        return () => {
            v.removeEventListener('play', onPlay);
            v.removeEventListener('pause', onPause);
            v.removeEventListener('loadedmetadata', onLoaded);
            v.removeEventListener('progress', onProgress);
            if (refRaf.current) cancelAnimationFrame(refRaf.current);
        };
    }, [duration]);

    // Controls handlers
    const togglePlay = async () => {
        const v = refVideo.current;
        if (!v) return;
        if (v.paused) {
            await v.play().catch(() => { });
        } else {
            v.pause();
        }
    };
    const seekTo = (t: number) => {
        const v = refVideo.current;
        if (!v) return;
        v.currentTime = Math.max(0, Math.min(duration || v.duration || 0, t));
    };
    const skip = (delta: number) => seekTo(currentTime + delta);
    const toggleMute = () => setIsMuted(m => !m);
    const changeRate = (r: number) => setPlaybackRate(r);
    const toggleFullscreen = async () => {
        const el = refVideo.current?.parentElement;
        if (!el) return;
        if (document.fullscreenElement) await document.exitFullscreen();
        else await el.requestFullscreen().catch(() => { });
    };
    const togglePip = async () => {
        const v = refVideo.current;
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

    const pct = useMemo(() => {
        const d = duration || refVideo.current?.duration || 0;
        if (!d) return 0;
        return Math.min(100, Math.max(0, (currentTime / d) * 100));
    }, [currentTime, duration]);
    const bufPct = useMemo(() => {
        const d = duration || refVideo.current?.duration || 0;
        if (!d) return 0;
        return Math.min(100, Math.max(0, (bufferedEnd / d) * 100));
    }, [bufferedEnd, duration]);

    // Progress bar interactions
    const onScrubMouse = (e: MouseEvent) => {
        // When dragging, the event target is window; fall back to progress bar ref
        const bar: HTMLElement | null = refDragging.current
            ? (refProgressBar.current as unknown as HTMLElement | null)
            : (e.currentTarget as HTMLElement | null);
        if (!bar || typeof bar.getBoundingClientRect !== 'function') return;
        const rect = bar.getBoundingClientRect();
        const x = Math.min(Math.max(e.clientX - rect.left, 0), rect.width);
        const pct = x / rect.width;
        setHoverPct(pct * 100);
        const d = duration || refVideo.current?.duration || 0;
        setHoverTime(pct * d);
        if (refDragging.current) {
            seekTo(pct * d);
        }
    };
    const onScrubMouseListener: EventListener = (ev) => onScrubMouse(ev as unknown as MouseEvent);
    const onScrubUpListener: EventListener = () => onScrubUp();
    const onScrubDown = (e: MouseEvent) => {
        refDragging.current = true;
        onScrubMouse(e);
        window.addEventListener('mousemove', onScrubMouseListener);
        window.addEventListener('mouseup', onScrubUpListener, { once: true });
    };
    const onScrubUp = () => {
        refDragging.current = false;
        window.removeEventListener('mousemove', onScrubMouseListener);
    };

    // Keyboard shortcuts
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if ((e.target as HTMLElement)?.tagName === 'INPUT') return;
            switch (e.key) {
                case ' ': e.preventDefault(); togglePlay(); break;
                case 'ArrowLeft': skip(-5); break;
                case 'ArrowRight': skip(5); break;
                case 'ArrowUp': setVolume(v => Math.min(1, v + 0.05)); break;
                case 'ArrowDown': setVolume(v => Math.max(0, v - 0.05)); break;
                case 'm': case 'M': toggleMute(); break;
                case 'f': case 'F': toggleFullscreen(); break;
            }
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
    }, [currentTime, duration]);

    const setAutoQuality = (auto: boolean) => {
        const p = refPlayer.current;
        if (!p) return;
        p.updateSettings({ streaming: { abr: { autoSwitchBitrate: { video: auto } } } });
        if (auto) setQualityIndex('auto');
    };
    const setManualQuality = (idx: number) => {
        const p = refPlayer.current;
        if (!p) return;
        setAutoQuality(false);
        try { p.setQualityFor('video', idx); } catch { /* ignore */ }
        setQualityIndex(idx);
    };

    return (
        <div id="dash-player">
            <div className="player-shell">
                {unsupportedMsg && (
                    <div className="warning">
                        {unsupportedMsg} Try H.264, or play in a browser/settings that support WebM.
                    </div>
                )}
                <video
                    ref={refVideo}
                    className="player-video"
                    muted={isMuted}
                    autoplay={autoplay}
                    playsInline
                    onClick={togglePlay}
                />

                {/* Controls */}
                <div className="controls">
                    {/* Progress bar */}
                    <div
                        className="progress-bar"
                        ref={refProgressBar}
                        onMouseDown={(e) => onScrubDown(e as unknown as MouseEvent)}
                        onMouseMove={(e) => onScrubMouse(e as unknown as MouseEvent)}
                        onMouseLeave={() => setHoverPct(null)}
                    >
                        <div className="progress-buffer" style={{ width: `${bufPct}%` }} />
                        <div className="progress-played" style={{ width: `${pct}%` }} />
                        {hoverPct !== null && (
                            <div className="progress-hover" style={{ left: `${hoverPct}%` }}>
                                <span>{formatTime(hoverTime)}</span>
                            </div>
                        )}
                    </div>

                    <div className="toolbar">
                        <div className="left">
                            <button className="btn" onClick={togglePlay} title={isPlaying ? 'Pause (Space)' : 'Play (Space)'}>
                                {isPlaying ? '❚❚' : '►'}
                            </button>
                            <button className="btn" onClick={() => skip(-5)} title="Back 5s (←)">-5s</button>
                            <button className="btn" onClick={() => skip(5)} title="Forward 5s (→)">+5s</button>
                            <span className="time">{formatTime(currentTime)} / {formatTime(duration || 0)}</span>
                        </div>
                        <div className="center">
                            <div className="menu">
                                <label>Speed</label>
                                <select value={playbackRate} onChange={e => changeRate(Number((e.target as HTMLSelectElement).value))}>
                                    {[0.5, 0.75, 1, 1.25, 1.5, 2].map(r => (
                                        <option key={r} value={r}>{r}x</option>
                                    ))}
                                </select>
                            </div>
                            <div className="menu">
                                <label>Quality</label>
                                <select value={qualityIndex as number | 'auto'} onChange={e => {
                                    const val = (e.target as HTMLSelectElement).value;
                                    if (val === 'auto') setAutoQuality(true);
                                    else setManualQuality(Number(val));
                                }}>
                                    <option value="auto">Auto</option>
                                    {qualities.map(q => (
                                        <option key={q.index} value={q.index}>{q.label}</option>
                                    ))}
                                </select>
                            </div>
                        </div>
                        <div className="right">
                            <div className="menu volume">
                                <button className="btn" onClick={toggleMute} title="Mute (M)">{isMuted ? '🔇' : '🔊'}</button>
                                <input type="range" min={0} max={1} step={0.01} value={isMuted ? 0 : volume} onInput={e => setVolume(Number((e.target as HTMLInputElement).value))} />
                            </div>
                            <button className="btn" onClick={togglePip} title="Picture in Picture">🗔</button>
                            <button className="btn" onClick={toggleFullscreen} title="Fullscreen (F)">⛶</button>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}


