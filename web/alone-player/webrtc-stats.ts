import type { StatsNerds } from "./types";

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
