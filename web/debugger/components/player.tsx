import { createSignal, createEffect } from "solid-js";
import PlayerCore from "../../alone-player/player-core";

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

    createEffect(() => {
        if (ref) {
            ref.srcObject = props.stream;
        }
    });

    createEffect(() => {
        if (ref) {
            props.onVideoElement?.(ref);
        }
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
            <div style={{ width: displayWidth() }}>
                <PlayerCore
                    autoplay
                    muted
                    controls
                    getPeerConnection={props.getPeerConnection}
                    onVideoElement={(video) => {
                        ref = video;
                        ref.srcObject = props.stream;
                        props.onVideoElement?.(video);
                        video.addEventListener("resize", () => {
                            setResolution(
                                `${video.videoWidth}x${video.videoHeight}`,
                            );
                        });
                    }}
                />
            </div>
        </>
    );
}
