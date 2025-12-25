type StatsNerds = {
    // transport
    bytesReceived: number;
    bytesSent: number;

    // candidate-pair && nominated
    currentRoundTripTime: number;

    // codec && video
    // id: "CIT01_45_level-idx=5;profile=0;tier=0"
    // mimeType: "video/AV1"
    // payloadType: 45
    // sdpFmtpLine: "level-idx=5;profile=0;tier=0"
    vcodec?: string;
    // codec && audio
    // mimeType: "audio/opus"
    // payloadType: 111
    // sdpFmtpLine: "minptime=10;useinbandfec=1"
    acodec?: string;

    // inbound-rtp && video
    frameWidth?: number;
    frameHeight?: number;
    framesPerSecond?: number;

    // inbound-rtp && audio
    // The <video> muted is stop decode audio
    // Because, muted `audioLevel` always `0`
    muted?: boolean;
    audioLevel?: number;

    // outbound-rtp && video
    // outbound-rtp && audio
};

export type { StatsNerds };
