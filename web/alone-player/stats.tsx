import type { Component } from "solid-js";
import type { StatsNerds } from "./types";
import "./stats.css";

interface Props {
    stats: StatsNerds;
    onClose: () => void;
}

const StatsForNerds: Component<Props> = (props) => {
    const formatBytes = (bytes: number): string => {
        if (bytes === 0) return "0 B";
        const k = 1024;
        const sizes = ["B", "KB", "MB", "GB"];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return `${parseFloat((bytes / k ** i).toFixed(2))} ${sizes[i]}`;
    };

    const convertSecondsToMilliseconds = (seconds: number): number => {
        return seconds ? Math.round(seconds * 1000) : 0;
    };

    return (
        <>
            <div class="title">
                <div>Stats for nerds</div>
                <span class="btn" on:click={() => props.onClose()}>
                    [X]
                </span>
            </div>

            <dl class="stats">
                <dt>Received: </dt>
                <dd>{formatBytes(props.stats.bytesReceived)}</dd>

                <dt>Sent: </dt>
                <dd>{formatBytes(props.stats.bytesSent)}</dd>

                <dt>Round Trip Time: </dt>
                <dd>
                    {convertSecondsToMilliseconds(
                        props.stats.currentRoundTripTime,
                    )}
                    ms
                </dd>

                <dt>Video Codec: </dt>
                <dd>{props.stats.vcodec ?? "-"}</dd>

                <dt>Audio Codec: </dt>
                <dd>{props.stats.acodec ?? "-"}</dd>

                <dt>Video Resolution: </dt>
                <dd>{`${props.stats.frameWidth}x${props.stats.frameHeight}@${props.stats.framesPerSecond}`}</dd>

                <dt>Audio volume: </dt>
                <dd>
                    {props.stats.muted
                        ? "muted"
                        : props.stats.audioLevel?.toFixed(2)}
                </dd>
            </dl>
        </>
    );
};

export default StatsForNerds;
