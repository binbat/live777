import { useCallback, useEffect, useState } from 'preact/hooks';
import { Badge, Button, Card, Loading, Select } from 'react-daisyui';
import { RefreshCw, Play, Calendar, Clock, Circle, CheckCircle, XCircle } from 'lucide-react';

import { useRefreshTimer } from '@/shared/hooks/use-refresh-timer';
import * as livemanApi from '../api';

export function RecordingsPage() {
    const [sessions, setSessions] = useState<livemanApi.RecordingSession[]>([]);
    const [streams, setStreams] = useState<string[]>([]);
    const [selectedStream, setSelectedStream] = useState<string>('all');
    const [selectedStatus, setSelectedStatus] = useState<string>('all');
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string>('');

    const fetchSessions = useCallback(async () => {
        try {
            setLoading(true);
            setError('');
            
            const query: livemanApi.RecordingSessionQuery = {};
            if (selectedStream && selectedStream !== 'all') {
                query.stream = selectedStream;
            }
            if (selectedStatus && selectedStatus !== 'all') {
                query.status = selectedStatus;
            }
            
            const response = await livemanApi.getRecordingSessions(query);
            setSessions(response.sessions);
            
            // Extract unique streams
            const uniqueStreams = [...new Set(response.sessions.map(s => s.stream))];
            setStreams(uniqueStreams);
            
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to fetch recording sessions');
            setSessions([]);
        } finally {
            setLoading(false);
        }
    }, [selectedStream, selectedStatus]);

    useEffect(() => {
        fetchSessions();
    }, [fetchSessions]);

    const { isRefreshing } = useRefreshTimer({
        callback: fetchSessions,
        interval: 30000, // 30 seconds
        immediate: false
    });

    const handleStreamChange = (event: Event) => {
        const target = event.target as HTMLSelectElement;
        setSelectedStream(target.value);
    };

    const handleStatusChange = (event: Event) => {
        const target = event.target as HTMLSelectElement;
        setSelectedStatus(target.value);
    };

    const handlePlayback = (session: livemanApi.RecordingSession) => {
        // Navigate to playback view
        const url = new URL(window.location.href);
        url.searchParams.set('view', 'playback');
        url.searchParams.set('stream', session.stream);
        if (session.id) {
            url.searchParams.set('sessionId', session.id);
        } else {
            // Fallback for backward compatibility
            url.searchParams.set('session', session.start_ts.toString());
        }
        window.history.pushState({}, '', url.toString());
        window.dispatchEvent(new PopStateEvent('popstate'));
    };

    const getStatusIcon = (status: string) => {
        switch (status) {
            case 'Active':
                return <Circle className="w-4 h-4 text-orange-500 fill-current" />;
            case 'Completed':
                return <CheckCircle className="w-4 h-4 text-green-500" />;
            case 'Failed':
                return <XCircle className="w-4 h-4 text-red-500" />;
            default:
                return <Circle className="w-4 h-4 text-gray-500" />;
        }
    };

    const getStatusColor = (status: string) => {
        switch (status) {
            case 'Active':
                return 'warning';
            case 'Completed':
                return 'success';
            case 'Failed':
                return 'error';
            default:
                return 'neutral';
        }
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

    if (loading && sessions.length === 0) {
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
                        Recording Sessions
                    </h2>
                    <Badge color="info" variant="outline">
                        {sessions.length} sessions
                    </Badge>
                </div>
                <Button
                    size="sm"
                    color="ghost"
                    onClick={fetchSessions}
                    disabled={isRefreshing}
                >
                    <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
                    Refresh
                </Button>
            </div>

            {/* Filters */}
            <div className="flex items-center gap-4">
                <div className="flex items-center gap-2">
                    <label className="label">
                        <span className="label-text font-medium">Stream:</span>
                    </label>
                    <Select
                        value={selectedStream}
                        onChange={handleStreamChange}
                        className="w-48"
                    >
                        <option value="all">All Streams</option>
                        {streams.map(stream => (
                            <option key={stream} value={stream}>
                                {stream}
                            </option>
                        ))}
                    </Select>
                </div>
                
                <div className="flex items-center gap-2">
                    <label className="label">
                        <span className="label-text font-medium">Status:</span>
                    </label>
                    <Select
                        value={selectedStatus}
                        onChange={handleStatusChange}
                        className="w-36"
                    >
                        <option value="all">All</option>
                        <option value="Active">Active</option>
                        <option value="Completed">Completed</option>
                        <option value="Failed">Failed</option>
                    </Select>
                </div>
            </div>

            {error && (
                <div className="alert alert-error">
                    <span>{error}</span>
                </div>
            )}

            {sessions.length === 0 ? (
                <Card className="p-8">
                    <div className="text-center text-gray-500">
                        <Calendar className="w-16 h-16 mx-auto mb-4 opacity-50" />
                        <p className="text-lg mb-2">No recording sessions found</p>
                        <p className="text-sm">Start recording streams to see sessions here</p>
                    </div>
                </Card>
            ) : (
                <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                    {sessions.map(session => (
                        <Card key={session.id || `${session.stream}-${session.start_ts}`} className="p-4">
                            <div className="flex items-center justify-between mb-3">
                                <h3 className="font-semibold text-lg truncate" title={session.stream}>
                                    {session.stream}
                                </h3>
                                <Badge 
                                    color={getStatusColor(session.status)} 
                                    variant="outline" 
                                    size="sm"
                                    className="flex items-center gap-1"
                                >
                                    {getStatusIcon(session.status)}
                                    {session.status}
                                </Badge>
                            </div>
                            
                            <div className="space-y-2 text-sm text-gray-600 mb-4">
                                <div className="flex items-center gap-2">
                                    <Clock className="w-4 h-4" />
                                    <span>Started: {formatTimestamp(session.start_ts)}</span>
                                </div>
                                {session.end_ts && (
                                    <div className="flex items-center gap-2">
                                        <Clock className="w-4 h-4" />
                                        <span>Ended: {formatTimestamp(session.end_ts)}</span>
                                    </div>
                                )}
                                <div className="flex items-center gap-2">
                                    <Clock className="w-4 h-4" />
                                    <span>Duration: {formatDuration(session.duration_ms)}</span>
                                </div>
                            </div>
                            
                            <Button
                                size="sm"
                                color="primary"
                                className="w-full"
                                onClick={() => handlePlayback(session)}
                                disabled={session.status === 'Failed'}
                            >
                                <Play className="w-4 h-4 mr-2" />
                                {session.status === 'Active' ? 'Watch Live' : 'Play Recording'}
                            </Button>
                        </Card>
                    ))}
                </div>
            )}
        </div>
    );
}