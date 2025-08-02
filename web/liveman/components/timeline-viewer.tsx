import { useMemo } from 'preact/hooks';
import { Badge } from 'react-daisyui';
import { Clock, Film } from 'lucide-react';

import { VirtualScroll } from '@/shared/components/virtual-scroll';
import * as livemanApi from '../api';

interface TimelineViewerProps {
    segments: livemanApi.Segment[];
    onSegmentClick?: (segment: livemanApi.Segment) => void;
    selectedSegmentId?: string;
    containerHeight?: number;
}

export function TimelineViewer({ 
    segments, 
    onSegmentClick, 
    selectedSegmentId, 
    containerHeight = 400 
}: TimelineViewerProps) {
    const timelineStats = useMemo(() => {
        if (segments.length === 0) return { totalDuration: 0, keyframeCount: 0 };
        
        const totalDuration = segments.reduce((sum, seg) => sum + seg.duration_ms, 0);
        const keyframeCount = segments.filter(seg => seg.is_keyframe).length;
        
        return { totalDuration, keyframeCount };
    }, [segments]);

    const renderSegment = (segment: livemanApi.Segment, index: number) => {
        const isSelected = selectedSegmentId === segment.id;
        const startTime = new Date(segment.start_ts / 1000).toLocaleTimeString();
        const duration = (segment.duration_ms / 1000).toFixed(2);

        return (
            <div
                className={`p-3 border-b border-gray-200 cursor-pointer hover:bg-gray-50 transition-colors ${
                    isSelected ? 'bg-blue-50 border-blue-200' : ''
                }`}
                onClick={() => onSegmentClick?.(segment)}
            >
                <div className="flex items-center justify-between">
                    <div className="flex items-center gap-3">
                        <div className="flex items-center gap-1">
                            {segment.is_keyframe ? (
                                <Film className="w-4 h-4 text-blue-500" />
                            ) : (
                                <Clock className="w-4 h-4 text-gray-400" />
                            )}
                            <span className="text-sm font-mono text-gray-600">
                                #{index + 1}
                            </span>
                        </div>
                        
                        <div className="flex flex-col">
                            <span className="text-sm font-medium">{startTime}</span>
                            <span className="text-xs text-gray-500">{duration}s</span>
                        </div>
                    </div>

                    <div className="flex items-center gap-2">
                        {segment.is_keyframe && (
                            <Badge color="info" size="xs">
                                Keyframe
                            </Badge>
                        )}
                        <span className="text-xs text-gray-400">
                            {segment.path.split('/').pop()}
                        </span>
                    </div>
                </div>
            </div>
        );
    };

    const formatDuration = (ms: number): string => {
        const totalSeconds = Math.floor(ms / 1000);
        const hours = Math.floor(totalSeconds / 3600);
        const minutes = Math.floor((totalSeconds % 3600) / 60);
        const seconds = totalSeconds % 60;
        
        if (hours > 0) {
            return `${hours}h ${minutes}m ${seconds}s`;
        }
        return `${minutes}m ${seconds}s`;
    };

    if (segments.length === 0) {
        return (
            <div className="text-center py-8 text-gray-500">
                <Clock className="w-12 h-12 mx-auto mb-4 opacity-50" />
                <p>No timeline segments available</p>
            </div>
        );
    }

    return (
        <div className="space-y-4">
            {/* Timeline Stats */}
            <div className="flex items-center gap-6 p-4 bg-gray-50 rounded-lg">
                <div className="stat">
                    <div className="stat-title text-xs">Total Segments</div>
                    <div className="stat-value text-lg">{segments.length}</div>
                </div>
                <div className="stat">
                    <div className="stat-title text-xs">Duration</div>
                    <div className="stat-value text-lg">{formatDuration(timelineStats.totalDuration)}</div>
                </div>
                <div className="stat">
                    <div className="stat-title text-xs">Keyframes</div>
                    <div className="stat-value text-lg">{timelineStats.keyframeCount}</div>
                </div>
            </div>

            {/* Virtual Scrolled Timeline */}
            <div className="border rounded-lg overflow-hidden">
                <div className="p-3 bg-gray-100 border-b">
                    <h4 className="font-medium text-sm">Timeline Segments</h4>
                    <p className="text-xs text-gray-600 mt-1">
                        Click on segments to jump to specific times
                    </p>
                </div>
                
                <VirtualScroll
                    items={segments}
                    itemHeight={70}
                    containerHeight={containerHeight}
                    renderItem={renderSegment}
                    overscan={3}
                />
            </div>
        </div>
    );
}