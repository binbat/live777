export const formatTime = (timestamp: number) =>
    new Date(timestamp).toLocaleString("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hourCycle: "h23",
    });

export function formatVideoTrackResolution(track: MediaStreamTrack): string {
    const settings = track.getSettings();
    if (!settings.width || !settings.height) {
        return "";
    }
    let fps = settings.frameRate;
    const fpsStr = fps ? `@${Math.round(fps)}` : "";
    return `${settings.width}x${settings.height}${fpsStr}`;
}

export const nextSeqId = (prefix: string, existingIds: string[]) => {
    let i = 0;
    let newId = `${prefix}${i}`;
    while (existingIds.includes(newId)) {
        i++;
        newId = `${prefix}${i}`;
    }
    return newId;
};
