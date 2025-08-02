import { useCallback, useEffect, useState } from 'preact/hooks';
import { Badge, Button, Card, Loading, Select } from 'react-daisyui';
import { RefreshCw, Play, Calendar, Clock } from 'lucide-react';

import { useRefreshTimer } from '@/shared/hooks/use-refresh-timer';
import * as livemanApi from '../api';

export function RecordingsPage() {
    const [recordings, setRecordings] = useState<string[]>([]);
    const [selectedStream, setSelectedStream] = useState<string>('');
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string>('');

    const fetchRecordings = useCallback(async () => {
        try {
            setLoading(true);
            setError('');
            const response = await livemanApi.getRecordingStreams();
            setRecordings(response.streams);
            if (response.streams.length > 0 && !selectedStream) {
                setSelectedStream(response.streams[0]);
            }
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to fetch recordings');
            setRecordings([]);
        } finally {
            setLoading(false);
        }
    }, [selectedStream]);

    useEffect(() => {
        fetchRecordings();
    }, []);

    const { isRefreshing } = useRefreshTimer({
        callback: fetchRecordings,
        interval: 30000, // 30 seconds
        immediate: false
    });

    const handleStreamChange = (event: Event) => {
        const target = event.target as HTMLSelectElement;
        setSelectedStream(target.value);
    };

    const handlePlayback = (streamId: string) => {
        // Navigate to playback view
        const url = new URL(window.location.href);
        url.searchParams.set('view', 'playback');
        url.searchParams.set('stream', streamId);
        window.history.pushState({}, '', url.toString());
        window.dispatchEvent(new PopStateEvent('popstate'));
    };

    if (loading && recordings.length === 0) {
        return (
            <div className="flex justify-center items-center h-64">
                <Loading variant="spinner" size="lg" />
            </div>
        );
    }

    return (
        <div className="space-y-6">
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-4">
                    <h2 className="text-2xl font-bold flex items-center gap-2">
                        <Calendar className="w-6 h-6" />
                        Recordings
                    </h2>
                    <Badge color="info" variant="outline">
                        {recordings.length} streams
                    </Badge>
                </div>
                <Button
                    size="sm"
                    color="ghost"
                    onClick={fetchRecordings}
                    disabled={isRefreshing}
                >
                    <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
                    Refresh
                </Button>
            </div>

            {error && (
                <div className="alert alert-error">
                    <span>{error}</span>
                </div>
            )}

            {recordings.length === 0 ? (
                <Card className="p-8">
                    <div className="text-center text-gray-500">
                        <Calendar className="w-16 h-16 mx-auto mb-4 opacity-50" />
                        <p className="text-lg mb-2">No recordings found</p>
                        <p className="text-sm">Start recording streams to see them here</p>
                    </div>
                </Card>
            ) : (
                <div className="space-y-4">
                    <div className="flex items-center gap-4">
                        <label className="label">
                            <span className="label-text font-medium">Select Stream:</span>
                        </label>
                        <Select
                            value={selectedStream}
                            onChange={handleStreamChange}
                            className="w-64"
                        >
                            {recordings.map(stream => (
                                <option key={stream} value={stream}>
                                    {stream}
                                </option>
                            ))}
                        </Select>
                    </div>

                    <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                        {recordings.map(stream => (
                            <Card key={stream} className="p-4">
                                <div className="flex items-center justify-between mb-3">
                                    <h3 className="font-semibold text-lg">{stream}</h3>
                                    <Badge color="success" variant="outline" size="sm">
                                        Recorded
                                    </Badge>
                                </div>
                                <div className="flex items-center gap-2 text-sm text-gray-600 mb-4">
                                    <Clock className="w-4 h-4" />
                                    <span>Available for playback</span>
                                </div>
                                <Button
                                    size="sm"
                                    color="primary"
                                    className="w-full"
                                    onClick={() => handlePlayback(stream)}
                                >
                                    <Play className="w-4 h-4 mr-2" />
                                    Play Recording
                                </Button>
                            </Card>
                        ))}
                    </div>
                </div>
            )}
        </div>
    );
}