import type { StatsNerds } from "./types";

export type VideoRtpDirection = "inbound" | "outbound";

export type VideoFpsSample = {
    frames: number;
    timestamp: number;
};

export type VideoFpsSamples = Record<string, VideoFpsSample>;

export type VideoFpsStats = {
    fps: number | null;
    samples: VideoFpsSamples;
};

function getFrameCount(
    report: RTCInboundRtpStreamStats | RTCOutboundRtpStreamStats,
    direction: VideoRtpDirection,
): number | undefined {
    if (direction === "inbound") {
        const inboundReport = report as RTCInboundRtpStreamStats;
        return inboundReport.framesDecoded ?? inboundReport.framesReceived;
    }
    const outboundReport = report as RTCOutboundRtpStreamStats;
    return outboundReport.framesEncoded ?? outboundReport.framesSent;
}

function calculateFps(
    report: RTCInboundRtpStreamStats | RTCOutboundRtpStreamStats,
    direction: VideoRtpDirection,
    previousSample: VideoFpsSample | null,
): { fps: number | null; sample: VideoFpsSample | null } {
    const framesPerSecond = report.framesPerSecond;
    const frames = getFrameCount(report, direction);
    const sample =
        typeof frames === "number"
            ? { frames, timestamp: report.timestamp }
            : null;

    if (typeof framesPerSecond === "number") {
        return { fps: framesPerSecond, sample };
    }
    if (
        !sample ||
        !previousSample ||
        sample.timestamp <= previousSample.timestamp
    ) {
        return { fps: null, sample };
    }

    const frameDelta = sample.frames - previousSample.frames;
    const timeDeltaSeconds =
        (sample.timestamp - previousSample.timestamp) / 1000;
    if (frameDelta < 0 || timeDeltaSeconds <= 0) {
        return { fps: null, sample };
    }
    return { fps: frameDelta / timeDeltaSeconds, sample };
}

export async function collectVideoRtpFps(
    peerConnection: RTCPeerConnection,
    direction: VideoRtpDirection,
    previousSamples: VideoFpsSamples,
): Promise<VideoFpsStats> {
    const stats = await peerConnection.getStats();
    const reportType = `${direction}-rtp`;
    const samples: VideoFpsSamples = {};
    let fps: number | null = null;

    stats.forEach((report) => {
        if (report.type !== reportType || report.kind !== "video") {
            return;
        }

        const nextResult = calculateFps(
            report as RTCInboundRtpStreamStats | RTCOutboundRtpStreamStats,
            direction,
            previousSamples[report.id] ?? null,
        );
        if (nextResult.sample) {
            samples[report.id] = nextResult.sample;
        }
        if (nextResult.fps !== null && (fps === null || nextResult.fps > fps)) {
            fps = nextResult.fps;
        }
    });

    return { fps, samples };
}

export async function collectWebRtcStats(
    peerConnection: RTCPeerConnection,
): Promise<StatsNerds> {
    const statsNerds: StatsNerds = {
        bytesReceived: 0,
        bytesSent: 0,
        currentRoundTripTime: 0,
    };

    const stats = await peerConnection.getStats();
    stats.forEach((report) => {
        if (report.type === "transport") {
            statsNerds.bytesReceived = report.bytesReceived ?? 0;
            statsNerds.bytesSent = report.bytesSent ?? 0;
        } else if (report.type === "codec") {
            const [kind, codec] = report.mimeType.toLowerCase().split("/");
            if (kind === "video") {
                statsNerds.vcodec = `${codec}@${report.sdpFmtpLine ?? ""}`;
            } else if (kind === "audio") {
                statsNerds.acodec = `${codec}@${report.sdpFmtpLine ?? ""}`;
            }
        } else if (report.type === "candidate-pair" && report.nominated) {
            statsNerds.currentRoundTripTime = report.currentRoundTripTime ?? 0;
        }

        if (report.type === "inbound-rtp" && report.kind === "video") {
            statsNerds.frameWidth = report.frameWidth;
            statsNerds.frameHeight = report.frameHeight;
            statsNerds.framesPerSecond = report.framesPerSecond;
        }

        if (report.type === "inbound-rtp" && report.kind === "audio") {
            statsNerds.audioLevel = report.audioLevel;
        }
    });

    return statsNerds;
}
