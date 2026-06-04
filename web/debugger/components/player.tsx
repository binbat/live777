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
}) {
    const [resolution, setResolution] = createSignal("");
    const [displayWidth, setDisplayWidth] = createSignal("320px");

    let ref: HTMLVideoElement | undefined;

    const handleResize = () => {
        if (!ref) return;
        setResolution(`${ref.videoWidth}x${ref.videoHeight}`);
    };

    onCleanup(() => {
        ref?.removeEventListener("resize", handleResize);
    });

    return (
        <>
            <h5>Raw Resolution: {resolution()}</h5>
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
                        ref?.removeEventListener("resize", handleResize);
                        ref = video;
                        props.onVideoElement?.(video);
                        video.addEventListener("resize", handleResize);
                        handleResize();
                    }}
                    getPeerConnection={props.getPeerConnection}
                />
            </div>
        </>
    );
}
