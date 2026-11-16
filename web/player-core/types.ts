type StatsNerds = {
    bytesReceived: number;
    bytesSent: number;
    bytesReceivedPerSecond?: number;
    bytesSentPerSecond?: number;
    currentRoundTripTime: number;
    vcodec?: string;
    acodec?: string;
    frameWidth?: number;
    frameHeight?: number;
    framesPerSecond?: number;
    muted?: boolean;
    audioLevel?: number;
    audioJitter?: number;
    audioPacketsLost?: number;
    audioPacketsReceived?: number;
    audioEnergy?: number;
    audioSamplesReceived?: number;
    audioConcealedSamples?: number;
    audioConcealmentEvents?: number;
};

export type { StatsNerds };
