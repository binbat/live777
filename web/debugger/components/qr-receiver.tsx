import { useSearchParams } from "@solidjs/router";
import { createSignal, onCleanup } from "solid-js";
import { QRCodeStreamDecoder } from "../../shared/qrcode-stream";
import subscribe from "./subscribe";

type State = "idle" | "subscribing" | "measuring";

export default function QrReceiver() {
    const [searchParams] = useSearchParams();
    const [state, setState] = createSignal<State>("idle");
    const [latency, setLatency] = createSignal<string>("");

    let videoRef: HTMLVideoElement | undefined;
    let decoder: QRCodeStreamDecoder | null = null;
    let stopWhep: (() => Promise<void>) | null = null;

    onCleanup(() => {
        stopAll();
    });

    const startSubscribe = async () => {
        setState("subscribing");
        try {
            [stopWhep] = await subscribe({
                url: `${location.origin}/whep/${searchParams.id || "-"}`,
                token: (searchParams.token as string) || "",
                onStream: (ms: MediaStream | null) => {
                    if (videoRef) videoRef.srcObject = ms;
                },
                onChannel: () => {},
                log: () => {},
            });
        } catch (e) {
            console.error(e);
            setState("idle");
        }
    };

    const startMeasure = () => {
        if (!videoRef) return;
        // video 元素需要有 width/height 属性供 QRCodeStreamDecoder 构造 OffscreenCanvas
        videoRef.width = videoRef.videoWidth || 320;
        videoRef.height = videoRef.videoHeight || 240;
        decoder = new QRCodeStreamDecoder(videoRef);
        decoder.addEventListener("latency", (e: CustomEvent<number>) => {
            setLatency(`${e.detail} ms`);
        });
        decoder.start();
        setState("measuring");
    };

    const stopAll = async () => {
        if (decoder) {
            decoder.stop();
            decoder = null;
        }
        if (stopWhep) {
            await stopWhep();
            stopWhep = null;
        }
        if (videoRef) {
            videoRef.srcObject = null;
        }
        setLatency("");
        setState("idle");
    };

    return (
        <fieldset>
            <legend>QR Receiver (WHEP)</legend>
            <div style="text-align: center;">
                <section>
                    <video
                        ref={videoRef}
                        style={{ width: "320px" }}
                        autoplay={true}
                        muted={true}
                        controls={true}
                    />
                </section>
                <section>
                    {state() === "idle" && (
                        <button type="button" onClick={startSubscribe}>
                            Start WHEP
                        </button>
                    )}
                    {state() === "subscribing" && (
                        <>
                            <button type="button" onClick={startMeasure}>
                                Measure
                            </button>
                            <button type="button" onClick={stopAll}>
                                Stop
                            </button>
                        </>
                    )}
                    {state() === "measuring" && (
                        <button type="button" onClick={stopAll}>
                            Stop
                        </button>
                    )}
                </section>
                <section>
                    <span>State: {state()}</span>
                    {latency() && <span> | Latency: <b>{latency()}</b></span>}
                </section>
            </div>
        </fieldset>
    );
}
