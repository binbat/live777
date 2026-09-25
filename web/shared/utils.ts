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

interface HttpErrorLike {
    status?: number;
    message?: string;
    json?: unknown;
    text?: unknown;
}

export const formatHttpError = (error: HttpErrorLike): string => {
    if (typeof error.json === 'object' && error.json !== null && 'error' in error.json) {
        const { error: bodyError } = error.json as { error?: unknown };
        if (typeof bodyError === 'string' && bodyError.length > 0) {
            return bodyError;
        }
    }

    if (typeof error.text === 'string' && error.text.length > 0) {
        return error.text;
    }

    const message = error.message ?? '';
    if (message.length > 0) {
        try {
            const body = JSON.parse(message) as unknown;
            if (typeof body === 'object' && body !== null && 'error' in body) {
                const { error: bodyError } = body as { error?: unknown };
                if (typeof bodyError === 'string' && bodyError.length > 0) {
                    return bodyError;
                }
            }
        } catch {
            // The response body is often plain text; fall back to the original message.
        }
        return message;
    }

    return typeof error.status === 'number' ? `Status: ${error.status}` : 'Unknown error';
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

/** Bits per second → human-readable byte rate, e.g. `106 KB/s`, `1.2 MB/s`. */
export const formatBitrate = (bps: number): string => {
    const bytesPerSecond = bps / 8;
    if (bytesPerSecond >= 1_000_000)
        return `${(bytesPerSecond / 1_000_000).toFixed(1)} MB/s`;
    if (bytesPerSecond >= 1_000)
        return `${Math.round(bytesPerSecond / 1_000)} KB/s`;
    return `${Math.round(bytesPerSecond)} B/s`;
};

/** Byte count → human-readable size, e.g. `512 KB`, `1.5 GB`. */
export const formatBytes = (bytes: number): string => {
    if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
    if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
    if (bytes >= 1_000) return `${Math.round(bytes / 1_000)} KB`;
    return `${bytes} B`;
};
