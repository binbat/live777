import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { Button, Card, Loading, Range, Input, Collapse } from 'react-daisyui';
import { ArrowLeft, Play, Pause, SkipBack, SkipForward, Clock, Calendar, List } from 'lucide-react';

import * as livemanApi from '../api';
import { TimelineViewer } from './timeline-viewer';

interface PlaybackPageProps {
    streamId: string;
    onBack: () => void;
}

export function PlaybackPage({ streamId, onBack }: PlaybackPageProps) {
    const videoRef = useRef<HTMLVideoElement>(null);
    const [timeline, setTimeline] = useState<livemanApi.TimelineResponse | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string>('');
    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(0);
    const [isPlaying, setIsPlaying] = useState(false);
    const [selectedRange, setSelectedRange] = useState<{ start?: number; end?: number }>({});
    const [selectedSegment, setSelectedSegment] = useState<string>('');

    // Load timeline data
    const fetchTimeline = useCallback(async () => {
        try {
            setLoading(true);
            setError('');
            const response = await livemanApi.getTimeline(streamId, { limit: 1000 });
            setTimeline(response);
            
            if (response.segments.length > 0) {
                const firstSegment = response.segments[0];
                const lastSegment = response.segments[response.segments.length - 1];
                setSelectedRange({
                    start: firstSegment.start_ts,
                    end: lastSegment.end_ts
                });
            }
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to load timeline');
        } finally {
            setLoading(false);
        }
    }, [streamId]);

    // Initialize DASH player
    const initializePlayer = useCallback(async () => {
        const video = videoRef.current;
        if (!video || !selectedRange.start || !selectedRange.end) return;

        try {
            // Import dash.js dynamically
            const dashjs = await import('dashjs');
            const player = dashjs.MediaPlayer().create();
            
            // Get MPD manifest URL
            const mpdUrl = `/api/record/${streamId}/mpd?start_ts=${selectedRange.start}&end_ts=${selectedRange.end}`;
            
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
    }, [streamId, selectedRange]);

    useEffect(() => {
        fetchTimeline();
    }, [fetchTimeline]);

    useEffect(() => {
        if (timeline && selectedRange.start && selectedRange.end) {
            initializePlayer();
        }
    }, [timeline, selectedRange, initializePlayer]);

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

    const handleSegmentClick = (segment: livemanApi.Segment) => {
        setSelectedSegment(segment.id);
        // Calculate the time offset within the selected range
        if (selectedRange.start) {
            const segmentOffset = (segment.start_ts - selectedRange.start) / 1000000; // Convert to seconds
            const video = videoRef.current;
            if (video && segmentOffset >= 0) {
                video.currentTime = segmentOffset;
            }
        }
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
                <h2 className="text-2xl font-bold">Playback: {streamId}</h2>
            </div>

            {error && (
                <div className="alert alert-error">
                    <span>{error}</span>
                </div>
            )}

            {timeline && (
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

                    {/* Timeline Viewer */}
                    <Card className="p-4">
                        <Collapse className="border rounded-lg">
                            <Collapse.Title className="text-lg font-medium">
                                <List className="w-5 h-5 mr-2" />
                                Timeline Segments ({timeline.segments.length})
                            </Collapse.Title>
                            <Collapse.Content>
                                <TimelineViewer
                                    segments={timeline.segments}
                                    onSegmentClick={handleSegmentClick}
                                    selectedSegmentId={selectedSegment}
                                    containerHeight={300}
                                />
                            </Collapse.Content>
                        </Collapse>
                    </Card>

                    {/* Timeline Information */}
                    <Card className="p-4">
                        <h3 className="text-lg font-semibold mb-4 flex items-center gap-2">
                            <Clock className="w-5 h-5" />
                            Timeline Information
                        </h3>
                        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                            <div className="stat">
                                <div className="stat-title">Total Segments</div>
                                <div className="stat-value text-2xl">{timeline.total_count}</div>
                            </div>
                            <div className="stat">
                                <div className="stat-title">Start Time</div>
                                <div className="stat-value text-sm">
                                    {timeline.segments.length > 0 && formatTimestamp(timeline.segments[0].start_ts)}
                                </div>
                            </div>
                            <div className="stat">
                                <div className="stat-title">End Time</div>
                                <div className="stat-value text-sm">
                                    {timeline.segments.length > 0 && formatTimestamp(timeline.segments[timeline.segments.length - 1].end_ts)}
                                </div>
                            </div>
                        </div>
                    </Card>

                    {/* Time Range Selection */}
                    <Card className="p-4">
                        <h3 className="text-lg font-semibold mb-4 flex items-center gap-2">
                            <Calendar className="w-5 h-5" />
                            Time Range Selection
                        </h3>
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                            <div>
                                <label className="label">
                                    <span className="label-text">Start Time</span>
                                </label>
                                <Input
                                    type="datetime-local"
                                    value={selectedRange.start ? new Date(selectedRange.start / 1000).toISOString().slice(0, 16) : ''}
                                    onChange={(e) => {
                                        const target = e.target as HTMLInputElement;
                                        const timestamp = new Date(target.value).getTime() * 1000;
                                        setSelectedRange(prev => ({ ...prev, start: timestamp }));
                                    }}
                                />
                            </div>
                            <div>
                                <label className="label">
                                    <span className="label-text">End Time</span>
                                </label>
                                <Input
                                    type="datetime-local"
                                    value={selectedRange.end ? new Date(selectedRange.end / 1000).toISOString().slice(0, 16) : ''}
                                    onChange={(e) => {
                                        const target = e.target as HTMLInputElement;
                                        const timestamp = new Date(target.value).getTime() * 1000;
                                        setSelectedRange(prev => ({ ...prev, end: timestamp }));
                                    }}
                                />
                            </div>
                        </div>
                        <div className="mt-4">
                            <Button 
                                color="primary" 
                                onClick={initializePlayer}
                                disabled={!selectedRange.start || !selectedRange.end}
                            >
                                Load Time Range
                            </Button>
                        </div>
                    </Card>
                </div>
            )}
        </div>
    );
}