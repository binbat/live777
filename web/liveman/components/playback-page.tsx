import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { Button, Card, Loading, Range } from 'react-daisyui';
import { ArrowLeft, Play, Pause, SkipBack, SkipForward, Clock, Calendar } from 'lucide-react';

import * as livemanApi from '../api';

interface PlaybackPageProps {
    streamId: string;
    sessionId?: string;
    onBack: () => void;
}

export function PlaybackPage({ streamId, sessionId, onBack }: PlaybackPageProps) {
    const videoRef = useRef<HTMLVideoElement>(null);
    const [session, setSession] = useState<livemanApi.RecordingSession | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string>('');
    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(0);
    const [isPlaying, setIsPlaying] = useState(false);

    // Load session data
    const fetchSession = useCallback(async () => {
        try {
            setLoading(true);
            setError('');
            
            if (sessionId) {
                // Find session by ID
                const response = await livemanApi.getRecordingSessions();
                const foundSession = response.sessions.find(s => s.id === sessionId);
                if (foundSession) {
                    setSession(foundSession);
                } else {
                    setError('Recording session not found');
                }
            } else {
                // Find latest session for this stream (fallback)
                const response = await livemanApi.getRecordingSessions({ 
                    stream: streamId,
                    limit: 1 
                });
                if (response.sessions.length > 0) {
                    setSession(response.sessions[0]);
                } else {
                    setError('No recording sessions found for this stream');
                }
            }
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to load recording session');
        } finally {
            setLoading(false);
        }
    }, [streamId, sessionId]);

    // Initialize DASH player
    const initializePlayer = useCallback(async () => {
        const video = videoRef.current;
        if (!video || !session?.id) return;

        try {
            // Import dash.js dynamically
            const dashjs = await import('dashjs');
            const player = dashjs.MediaPlayer().create();
            
            // Get MPD manifest URL
            const mpdUrl = `/api/record/sessions/${session.id}/mpd`;
            
            player.initialize(video, mpdUrl, false);
            
            player.on(dashjs.MediaPlayer.events.PLAYBACK_STARTED, () => {
                setIsPlaying(true);
            });
            
            player.on(dashjs.MediaPlayer.events.PLAYBACK_PAUSED, () => {
                setIsPlaying(false);
            });
            
            player.on(dashjs.MediaPlayer.events.PLAYBACK_TIME_UPDATED, () => {
                setCurrentTime(video.currentTime);
            });

            player.on(dashjs.MediaPlayer.events.STREAM_INITIALIZED, () => {
                setDuration(video.duration || 0);
            });

        } catch (err) {
            setError('Failed to initialize video player');
            console.error('Player initialization error:', err);
        }
    }, [session]);

    useEffect(() => {
        fetchSession();
    }, [fetchSession]);

    useEffect(() => {
        if (session) {
            initializePlayer();
        }
    }, [session, initializePlayer]);

    const handlePlayPause = () => {
        const video = videoRef.current;
        if (!video) return;

        if (isPlaying) {
            video.pause();
        } else {
            video.play();
        }
    };

    const handleSeek = (event: Event) => {
        const video = videoRef.current;
        const target = event.target as HTMLInputElement;
        if (!video) return;

        const newTime = parseFloat(target.value);
        video.currentTime = newTime;
        setCurrentTime(newTime);
    };

    const handleSkip = (seconds: number) => {
        const video = videoRef.current;
        if (!video) return;

        video.currentTime = Math.max(0, Math.min(duration, video.currentTime + seconds));
    };

    const formatTime = (timeInSeconds: number): string => {
        const hours = Math.floor(timeInSeconds / 3600);
        const minutes = Math.floor((timeInSeconds % 3600) / 60);
        const seconds = Math.floor(timeInSeconds % 60);
        
        if (hours > 0) {
            return `${hours}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
        }
        return `${minutes}:${seconds.toString().padStart(2, '0')}`;
    };

    const formatTimestamp = (timestamp: number): string => {
        return new Date(timestamp / 1000).toLocaleString();
    };

    const formatDuration = (durationMs: number | null): string => {
        if (!durationMs) return 'N/A';
        const seconds = Math.floor(durationMs / 1000);
        const minutes = Math.floor(seconds / 60);
        const hours = Math.floor(minutes / 60);
        
        if (hours > 0) {
            return `${hours}h ${minutes % 60}m ${seconds % 60}s`;
        } else if (minutes > 0) {
            return `${minutes}m ${seconds % 60}s`;
        } else {
            return `${seconds}s`;
        }
    };

    if (loading) {
        return (
            <div className="flex justify-center items-center h-64">
                <Loading variant="spinner" size="lg" />
            </div>
        );
    }

    return (
        <div className="space-y-6">
            <div className="flex items-center gap-4">
                <Button size="sm" color="ghost" onClick={onBack}>
                    <ArrowLeft className="w-4 h-4" />
                    Back to Recordings
                </Button>
                <h2 className="text-2xl font-bold">
                    {session?.status === 'Active' ? 'Live Playback' : 'Recording Playback'}: {streamId}
                </h2>
            </div>

            {error && (
                <div className="alert alert-error">
                    <span>{error}</span>
                </div>
            )}

            {session && (
                <div className="space-y-4">
                    {/* Video Player */}
                    <Card className="p-4">
                        <div className="aspect-video bg-black rounded-lg overflow-hidden mb-4">
                            <video
                                ref={videoRef}
                                className="w-full h-full"
                                controls={false}
                                autoPlay={false}
                            />
                        </div>

                        {/* Player Controls */}
                        <div className="space-y-4">
                            <div className="flex items-center gap-2">
                                <span className="text-sm font-mono min-w-[60px]">
                                    {formatTime(currentTime)}
                                </span>
                                <Range
                                    min={0}
                                    max={duration}
                                    value={currentTime}
                                    onChange={handleSeek}
                                    className="flex-1"
                                />
                                <span className="text-sm font-mono min-w-[60px]">
                                    {formatTime(duration)}
                                </span>
                            </div>

                            <div className="flex items-center justify-center gap-2">
                                <Button size="sm" onClick={() => handleSkip(-10)}>
                                    <SkipBack className="w-4 h-4" />
                                    10s
                                </Button>
                                <Button size="lg" color="primary" onClick={handlePlayPause}>
                                    {isPlaying ? <Pause className="w-6 h-6" /> : <Play className="w-6 h-6" />}
                                </Button>
                                <Button size="sm" onClick={() => handleSkip(10)}>
                                    10s
                                    <SkipForward className="w-4 h-4" />
                                </Button>
                            </div>
                        </div>
                    </Card>

                    {/* Session Information */}
                    <Card className="p-4">
                        <h3 className="text-lg font-semibold mb-4 flex items-center gap-2">
                            <Calendar className="w-5 h-5" />
                            Session Information
                        </h3>
                        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                            <div className="stat">
                                <div className="stat-title">Status</div>
                                <div className="stat-value text-2xl">{session.status}</div>
                            </div>
                            <div className="stat">
                                <div className="stat-title">Start Time</div>
                                <div className="stat-value text-sm">
                                    {formatTimestamp(session.start_ts)}
                                </div>
                            </div>
                            <div className="stat">
                                <div className="stat-title">Duration</div>
                                <div className="stat-value text-sm">
                                    {formatDuration(session.duration_ms)}
                                </div>
                            </div>
                        </div>
                        {session.end_ts && (
                            <div className="mt-4">
                                <div className="stat">
                                    <div className="stat-title">End Time</div>
                                    <div className="stat-value text-sm">
                                        {formatTimestamp(session.end_ts)}
                                    </div>
                                </div>
                            </div>
                        )}
                    </Card>
                </div>
            )}
        </div>
    );
}