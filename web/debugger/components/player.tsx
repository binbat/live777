import { PlayerSurface } from "player-core";
import { createSignal, onCleanup } from "solid-js";

const DisplayWidthOptions = [
    { value: "320px", text: "320px" },
    { value: "480px", text: "480px" },
    { value: "600px", text: "600px" },
    { value: "1280px", text: "1280px" },
    { value: "1920px", text: "1920px" },
    { value: "", text: "auto" },
];

export default function Player(props: {
    stream: MediaStream;
    onVideoElement?: (video: HTMLVideoElement) => void;
    getPeerConnection?: () => RTCPeerConnection | null;
    showRenderFps?: boolean;
}) {
    const [resolution, setResolution] = createSignal("");
    const [renderFps, setRenderFps] = createSignal<number | null>(null);
    const [displayWidth, setDisplayWidth] = createSignal("320px");

    let ref: HTMLVideoElement | undefined;
    let renderFpsCallbackId: number | null = null;
    let renderFpsToken = 0;
    let windowPresentedFrames: number | null = null;
    let windowStartedAt: DOMHighResTimeStamp | null = null;
    let lastFrameTime: DOMHighResTimeStamp | null = null;
    let staleTimer: ReturnType<typeof setInterval> | null = null;

    const handleResize = () => {
        if (!ref) return;
        setResolution(`${ref.videoWidth}x${ref.videoHeight}`);
    };

    const stopRenderFps = () => {
        renderFpsToken += 1;
        if (ref && renderFpsCallbackId !== null) {
            ref.cancelVideoFrameCallback(renderFpsCallbackId);
        }
        renderFpsCallbackId = null;
        windowPresentedFrames = null;
        windowStartedAt = null;
        lastFrameTime = null;
        if (staleTimer) {
            clearInterval(staleTimer);
            staleTimer = null;
        }
        setRenderFps(null);
    };

    const startRenderFps = () => {
        if (!ref || !props.showRenderFps) return;
        stopRenderFps();
        const video = ref;
        const token = renderFpsToken;

        const onFrame: VideoFrameRequestCallback = (now, metadata) => {
            if (token !== renderFpsToken) return;
            if (windowPresentedFrames === null || windowStartedAt === null) {
                windowPresentedFrames = metadata.presentedFrames;
                windowStartedAt = now;
            } else if (
                metadata.presentedFrames > windowPresentedFrames &&
                now - windowStartedAt >= 1000
            ) {
                const frameCount =
                    metadata.presentedFrames - windowPresentedFrames;
                setRenderFps((frameCount * 1000) / (now - windowStartedAt));
                windowPresentedFrames = metadata.presentedFrames;
                windowStartedAt = now;
            }
            lastFrameTime = now;
            renderFpsCallbackId = video.requestVideoFrameCallback(onFrame);
        };

        renderFpsCallbackId = video.requestVideoFrameCallback(onFrame);
        staleTimer = setInterval(() => {
            if (
                lastFrameTime !== null &&
                performance.now() - lastFrameTime > 2000
            ) {
                windowPresentedFrames = null;
                windowStartedAt = null;
                setRenderFps(null);
            }
        }, 1000);
    };

    onCleanup(() => {
        stopRenderFps();
        ref?.removeEventListener("resize", handleResize);
    });

    return (
        <>
            <h5>
                Raw Resolution: {resolution()}
                {props.showRenderFps && `@${renderFps()?.toFixed(1) ?? "--"}`}
            </h5>
            <label>
                Video Width:
                <select
                    value={displayWidth()}
                    onChange={(e) => {
                        setDisplayWidth(e.target.value);
                    }}
                >
                    {DisplayWidthOptions.map((o) => (
                        <option value={o.value}>{o.text}</option>
                    ))}
                </select>
            </label>
            <br />
            <div style={{ width: displayWidth(), margin: "0 auto" }}>
                <PlayerSurface
                    stream={props.stream}
                    autoplay
                    muted
                    controls
                    onVideoElement={(video) => {
                        if (ref === video) return;
                        stopRenderFps();
                        ref?.removeEventListener("resize", handleResize);
                        ref = video;
                        props.onVideoElement?.(video);
                        video.addEventListener("resize", handleResize);
                        handleResize();
                        startRenderFps();
                    }}
                    getPeerConnection={props.getPeerConnection}
                />
            </div>
        </>
    );
}
