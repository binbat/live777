export const formatTime = (timestamp: number) => new Date(timestamp).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23'
});

export const formatVideoTrackResolution = (track: MediaStreamTrack): string => {
    // firefox@127 returns empty object for this
    const { width, height, frameRate } = track.getSettings();
    if (!width || !height) return '';
    if (!frameRate) return `${width}x${height}`;
    return `${width}x${height}@${Math.round(frameRate)}`;
};

export const nextSeqId = (prefix: string, existingIds: string[]) => {
    let i = 0;
    let newId = `${prefix}${i}`;
    while (existingIds.includes(newId)) {
        i++;
        newId = `${prefix}${i}`;
    }
    return newId;
};
