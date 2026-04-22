import { useSearchParams } from "@solidjs/router";
import { createSignal, onCleanup, onMount } from "solid-js";
import { WHIPClient } from "@binbat/whip-whep/whip.js";
import { QRCodeStream } from "../../shared/qrcode-stream";
import convertSessionDescription from "./sdp";

type State = "idle" | "previewing" | "publishing";

const PREVIEW_SIZE = 320;

export default function QrSender() {
    const [searchParams] = useSearchParams();
    const [state, setState] = createSignal<State>("idle");

    let canvasRef: HTMLCanvasElement | undefined;
    let videoRef: HTMLVideoElement | undefined;

    let qrStream: QRCodeStream | null = null;
    let whipClient: WHIPClient | null = null;
    let pc: RTCPeerConnection | null = null;
    let published = false;

    onMount(() => {
        canvasRef!.width = PREVIEW_SIZE;
        canvasRef!.height = PREVIEW_SIZE;
    });

    onCleanup(() => {
        stopAll();
    });

    const startPreview = () => {
        if (!qrStream) {
            qrStream = new QRCodeStream(canvasRef!);
        }
        const ms = qrStream.capture();
        videoRef!.srcObject = ms;
        setState("previewing");
    };

    const publish = async () => {
        if (!qrStream) return;
        const ms = qrStream.capture();

        pc = new RTCPeerConnection();
        pc.addTransceiver(ms.getVideoTracks()[0], { direction: "sendonly" });

        whipClient = new WHIPClient();
        published = false;
        // biome-ignore lint/suspicious/noExplicitAny: whip-whep.js uses any
        whipClient.onAnswer = (answer: any) =>
            convertSessionDescription(answer, "", "");

        setState("publishing");
        try {
            const url = `${location.origin}/whip/${searchParams.id || "-"}`;
            await whipClient.publish(pc, url, (searchParams.token as string) || "");
            published = true;
        } catch (e) {
            console.error(e);
            await stopAll();
        }
    };

    const stopAll = async () => {
        if (whipClient) {
            if (published) {
                await whipClient.stop();
            }
            whipClient = null;
        }
        published = false;
        if (pc) {
            pc.close();
            pc = null;
        }
        if (qrStream) {
            qrStream.stop();
            qrStream = null;
        }
        if (videoRef) {
            videoRef.srcObject = null;
        }
        setState("idle");
    };

    return (
        <fieldset>
            <legend>QR Sender (WHIP)</legend>
            <div style="text-align: center;">
                <canvas ref={canvasRef} style="display: none;" />
                <section style={{ width: `${PREVIEW_SIZE}px`, height: `${PREVIEW_SIZE}px` }}>
                    <video
                        ref={videoRef}
                        style={{
                            width: "100%",
                            height: "100%",
                            "object-fit": "contain",
                            "background-color": "#ffffff",
                        }}
                        autoplay={true}
                        muted={true}
                    />
                </section>
                <section>
                    {state() === "idle" && (
                        <button type="button" onClick={startPreview}>
                            Start QR
                        </button>
                    )}
                    {state() === "previewing" && (
                        <>
                            <button type="button" onClick={publish}>
                                Publish
                            </button>
                            <button type="button" onClick={stopAll}>
                                Stop
                            </button>
                        </>
                    )}
                    {state() === "publishing" && (
                        <button type="button" onClick={stopAll}>
                            Stop
                        </button>
                    )}
                </section>
                <section>
                    <span>State: {state()}</span>
                </section>
            </div>
        </fieldset>
    );
}
