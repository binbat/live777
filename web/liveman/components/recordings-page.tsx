import { useCallback, useEffect, useState } from 'preact/hooks';
import { Badge, Button, Card, Loading } from 'react-daisyui';
import { RefreshCw, Calendar } from 'lucide-react';
import * as livemanApi from '../api';

export function RecordingsPage() {
    const [streams, setStreams] = useState<string[]>([]);
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
        } catch (e) {
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
            res.sort((a, b) => {
                if (a.year !== b.year) return b.year - a.year;
                if (a.month !== b.month) return b.month - a.month;
                return b.day - a.day;
            });
            setIndexEntries(res);
        } catch (e) {
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

    const selectStream = (s: string) => setSelectedStream(s);
    const playMpd = (mpd: string) => {
        const url = new URL(window.location.href);
        url.searchParams.set('view', 'recordings');
        // 直接打开播放 modal（在此页内实现 modal 播放器亦可后续追加）
        url.searchParams.set('stream', selectedStream);
        url.searchParams.set('mpd', mpd);
        window.history.pushState({}, '', url.toString());
        window.dispatchEvent(new PopStateEvent('popstate'));
        // 简化：直接新开标签访问 MPD 以便快速验证
        window.open(livemanApi.getSegmentUrl(mpd), '_blank');
    };

    if (loading && streams.length === 0) {
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
                        {indexEntries.length} entries
                    </Badge>
                </div>
                <Button size="sm" color="ghost" onClick={() => { fetchStreams(); fetchIndex(); }}>
                    <RefreshCw className="w-4 h-4" />
                    Refresh
                </Button>
            </div>

            <div className="flex flex-wrap gap-2">
                {streams.map((s) => (
                    <Button key={s} size="sm" color={s === selectedStream ? 'primary' : 'ghost'} onClick={() => selectStream(s)}>
                        {s}
                    </Button>
                ))}
            </div>

            {error && (
                <div className="alert alert-error">
                    <span>{error}</span>
                </div>
            )}

            {selectedStream && indexEntries.length === 0 ? (
                <Card className="p-8">
                    <div className="text-center text-gray-500">
                        <Calendar className="w-16 h-16 mx-auto mb-4 opacity-50" />
                        <p className="text-lg mb-2">No recordings</p>
                        <p className="text-sm">Start recording streams to see them here</p>
                    </div>
                </Card>
            ) : (
                <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                    {indexEntries.map((e) => (
                        <Card key={`${e.year}-${e.month}-${e.day}-${e.mpd_path}`} className="p-4">
                            <div className="flex items-center justify-between mb-3">
                                <h3 className="font-semibold text-lg truncate">
                                    {selectedStream}
                                </h3>
                                <Badge color="success" variant="outline" size="sm">
                                    {`${e.year}-${String(e.month).padStart(2, '0')}-${String(e.day).padStart(2, '0')}`}
                                </Badge>
                            </div>
                            <div className="flex gap-2">
                                <Button size="sm" color="primary" className="flex-1" onClick={() => playMpd(e.mpd_path)}>Play</Button>
                                <Button size="sm" color="ghost" onClick={() => window.open(livemanApi.getSegmentUrl(e.mpd_path), '_blank')}>Open MPD</Button>
                            </div>
                        </Card>
                    ))}
                </div>
            )}
        </div>
    );
}