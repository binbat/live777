export type QrLatencySample = {
    latencyMs: number;
    receivedAtMs: number;
};

export type QrLatencyDisplay = {
    state: "waiting" | "live" | "stale" | "lost";
    status: string;
    value: string;
};

const StaleAfterMs = 1_000;
const LostAfterMs = 3_000;

export function getQrLatencyDisplay(
    nowMs: number,
    measurementStartedAtMs: number,
    latestSample: QrLatencySample | null,
): QrLatencyDisplay {
    if (!latestSample) {
        if (nowMs - measurementStartedAtMs >= LostAfterMs) {
            return {
                state: "lost",
                status: "QR not detected",
                value: "-- ms",
            };
        }

        return {
            state: "waiting",
            status: "Waiting for QR",
            value: "-- ms",
        };
    }

    const ageMs = Math.max(0, nowMs - latestSample.receivedAtMs);

    if (ageMs >= LostAfterMs) {
        return {
            state: "lost",
            status: "QR detection lost",
            value: "-- ms",
        };
    }

    if (ageMs >= StaleAfterMs) {
        return {
            state: "stale",
            status: `${(ageMs / 1_000).toFixed(1)}s stale`,
            value: `${latestSample.latencyMs} ms`,
        };
    }

    return {
        state: "live",
        status: "Live",
        value: `${latestSample.latencyMs} ms`,
    };
}
