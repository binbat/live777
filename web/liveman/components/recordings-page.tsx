import { useCallback, useContext, useEffect, useMemo, useState } from 'preact/hooks';
import { Badge, Button, Card, Input, Loading, Select, Tooltip } from 'react-daisyui';
import { RefreshCw, Calendar, Search, Play, Link2, Copy } from 'lucide-react';
import * as livemanApi from '../api';
import { TokenContext } from '@/shared/context';

function formatYearMonthDay(timestamp: string): string {
    const date = new Date(parseInt(timestamp) * 1000);
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, '0');
    const day = String(date.getDate()).padStart(2, '0');

    return `${year}-${month}-${day}`;
}

function formatDateTime(timestamp: string): string {
    const date = new Date(parseInt(timestamp) * 1000);
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, '0');
    const day = String(date.getDate()).padStart(2, '0');
    const hours = String(date.getHours()).padStart(2, '0');
    const minutes = String(date.getMinutes()).padStart(2, '0');
    const seconds = String(date.getSeconds()).padStart(2, '0');

    return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}`;
}

function getFileName(path: string) {
    try {
        const idx = path.lastIndexOf('/');
        return idx >= 0 ? path.slice(idx + 1) : path;
    } catch {
        return path;
    }
}

export function RecordingsPage() {
    const tokenContext = useContext(TokenContext);
    const [streams, setStreams] = useState<string[]>([]);
    const [streamFilter, setStreamFilter] = useState('');
    const [selectedStream, setSelectedStream] = useState<string>('');
    const [indexEntries, setIndexEntries] = useState<livemanApi.RecordingIndexEntry[]>([]);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string>('');

    const fetchStreams = useCallback(async () => {
        try {
            setLoading(true);
            setError('');
            const res = await livemanApi.getRecordingIndexStreams();
            setStreams(res);
            if (!selectedStream && res.length > 0) {
                setSelectedStream(res[0]);
            }
        } catch {
            setError('Failed to fetch streams');
        } finally {
            setLoading(false);
        }
    }, [selectedStream]);

    const fetchIndex = useCallback(async () => {
        if (!selectedStream) {
            setIndexEntries([]);
            return;
        }
        try {
            setLoading(true);
            setError('');
            const res = await livemanApi.getRecordingIndexByStream(selectedStream);
            res.sort((a, b) => parseInt(b.record) - parseInt(a.record));
            setIndexEntries(res);
        } catch {
            setError('Failed to fetch recording index');
        } finally {
            setLoading(false);
        }
    }, [selectedStream]);

    useEffect(() => {
        fetchStreams();
    }, [fetchStreams]);

    useEffect(() => {
        fetchIndex();
    }, [fetchIndex]);

    const playMpd = (mpd: string) => {
        const params = new URLSearchParams();
        params.set('mpd', mpd);
        if (tokenContext.token) params.set('token', tokenContext.token);
        const url = new URL(`/tools/dash.html?${params.toString()}`, location.origin);
        window.open(url.toString(), '_blank');
    };

    const copyToClipboard = async (text: string) => {
        try { await navigator.clipboard.writeText(text); } catch { /* ignore */ }
    };

    const filteredStreams = useMemo(() => {
        const f = streamFilter.trim().toLowerCase();
        if (!f) return streams;
        return streams.filter(s => s.toLowerCase().includes(f));
    }, [streams, streamFilter]);

    const groupedByDay = useMemo(() => {
        const groups = new Map<string, livemanApi.RecordingIndexEntry[]>();
        for (const e of indexEntries) {
            const key = formatYearMonthDay(e.record);
            const arr = groups.get(key) ?? [];
            arr.push(e);
            groups.set(key, arr);
        }
        // sort each group by day desc
        for (const [, arr] of groups) arr.sort((a, b) => parseInt(b.record) - parseInt(a.record));
        // return sorted keys desc by year-month
        return Array.from(groups.entries()).sort((a, b) => b[0].localeCompare(a[0]));
    }, [indexEntries]);

    if (loading && streams.length === 0) {
        return (
            <div className="flex justify-center items-center h-64">
                <Loading variant="spinner" size="lg" />
            </div>
        );
    }

    return (
        <div className="space-y-6">
            {/* Header */}
            <div className="flex items-center justify-between gap-4">
                <div className="flex items-center gap-3">
                    <h2 className="text-2xl font-bold flex items-center gap-2">
                        <Calendar className="w-6 h-6" />
                        Recordings
                    </h2>
                    <Badge color="info" variant="outline">{indexEntries.length} entries</Badge>
                </div>
                <Button size="sm" color="ghost" onClick={() => { fetchStreams(); fetchIndex(); }}>
                    <RefreshCw className="w-4 h-4" />
                    Refresh
                </Button>
            </div>

            {/* Stream picker */}
            <div className="flex flex-wrap items-center gap-3">
                <div className="relative">
                    <Input
                        size="sm"
                        value={streamFilter}
                        onInput={e => setStreamFilter((e.target as HTMLInputElement).value)}
                        placeholder="Search streams"
                        className="pl-8"
                    />
                    <Search className="w-4 h-4 absolute left-2 top-1/2 -translate-y-1/2 text-gray-500" />
                </div>
                <Select size="sm" value={selectedStream} onChange={e => setSelectedStream((e.target as HTMLSelectElement).value)}>
                    {filteredStreams.map(s => <option value={s}>{s}</option>)}
                </Select>
            </div>

            {error && (
                <div className="alert alert-error">
                    <span>{error}</span>
                </div>
            )}

            {/* Empty state */}
            {selectedStream && indexEntries.length === 0 ? (
                <Card className="p-8">
                    <div className="text-center text-gray-500">
                        <Calendar className="w-16 h-16 mx-auto mb-4 opacity-50" />
                        <p className="text-lg mb-2">No recordings</p>
                        <p className="text-sm">Recordings for the selected stream will appear here.</p>
                    </div>
                </Card>
            ) : null}

            {/* Grouped list */}
            {groupedByDay.map(([ymd, list]) => (
                <Card key={ymd} className="p-4">
                    <div className="flex items-center justify-between mb-3">
                        <div className="flex items-center gap-2">
                            <span className="text-lg font-semibold">{ymd}</span>
                            <Badge color="ghost">{list.length}</Badge>
                        </div>
                        <span className="text-sm opacity-70 truncate">{selectedStream}</span>
                    </div>
                    <div className="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
                        {list.map(e => (
                            <div key={ e.record } className="border border-base-200 rounded-lg p-3 flex flex-col gap-2">
                                <div className="flex items-center justify-between">
                                    <span className="font-medium">{ formatDateTime(e.record)}</span>
                                    <span className="text-xs opacity-70 font-mono truncate" title={e.mpd_path}>{getFileName(e.mpd_path)}</span>
                                </div>
                                <div className="flex items-center gap-2">
                                    <Button size="sm" color="primary" className="flex-1" onClick={() => playMpd(e.mpd_path)}>
                                        <Play className="w-4 h-4" />
                                        Play
                                    </Button>
                                    <Tooltip message="Copy DASH player link">
                                        <Button size="sm" color="ghost" onClick={() => copyToClipboard(new URL(`/tools/dash.html?mpd=${encodeURIComponent(e.mpd_path)}${tokenContext.token ? `&token=${encodeURIComponent(tokenContext.token)}` : ''}`, location.origin).toString())}>
                                            <Link2 className="w-4 h-4" />
                                        </Button>
                                    </Tooltip>
                                    <Tooltip message="Copy MPD URL">
                                        <Button size="sm" color="ghost" onClick={() => copyToClipboard(new URL(livemanApi.getSegmentUrl(e.mpd_path), location.origin).toString())}>
                                            <Copy className="w-4 h-4" />
                                        </Button>
                                    </Tooltip>
                                </div>
                            </div>
                        ))}
                    </div>
                </Card>
            ))}
        </div>
    );
}
