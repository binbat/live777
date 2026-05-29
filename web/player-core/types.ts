type StatsNerds = {
    bytesReceived: number;
    bytesSent: number;
    currentRoundTripTime: number;
    vcodec?: string;
    acodec?: string;
    frameWidth?: number;
    frameHeight?: number;
    framesPerSecond?: number;
    muted?: boolean;
    audioLevel?: number;
};

export type { StatsNerds };
