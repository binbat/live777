export const formatTime = (timestamp: number) => new Date(timestamp).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23'
})

export const formatVideoTrackResolution = (track: MediaStreamTrack): string => {
    const { width, height, frameRate } = track.getSettings()
    return `${width}x${height}@${frameRate}`
}
