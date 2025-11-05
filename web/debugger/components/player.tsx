import { createSignal, onMount } from "solid-js";

const DisplayWidthOptions = [
    { value: "320px", text: "320px" },
    { value: "480px", text: "480px" },
    { value: "600px", text: "600px" },
    { value: "1280px", text: "1280px" },
    { value: "1920px", text: "1920px" },
    { value: "", text: "auto" },
];

export default function Player(props: { stream: MediaStream }) {
    const [resolution, setResolution] = createSignal("");
    const [displayWidth, setDisplayWidth] = createSignal("320px");

    let ref: HTMLVideoElement | undefined;

    onMount(() => {
        if (ref) {
            ref.srcObject = props.stream;
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
            <video
                ref={ref}
                style={{ width: displayWidth() }}
                onResize={(e) => {
                    const video = e.target as HTMLVideoElement;
                    setResolution(`${video.videoWidth}x${video.videoHeight}`);
                }}
                autoplay={true}
                muted={true}
                controls={true}
            />
        </>
    );
}
