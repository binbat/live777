import { useSearchParams } from "@solidjs/router";
import { createSignal, onCleanup, onMount } from "solid-js";
import { WHIPClient } from "@binbat/whip-whep/whip.js";
import { QRCodeStream } from "../../shared/qrcode-stream";
import convertSessionDescription from "./sdp";

type State = "idle" | "previewing" | "publishing";

export default function QrSender() {
    const [searchParams] = useSearchParams();
    const [state, setState] = createSignal<State>("idle");

    let canvasRef: HTMLCanvasElement | undefined;
    let videoRef: HTMLVideoElement | undefined;

    let qrStream: QRCodeStream | null = null;
    let whipClient: WHIPClient | null = null;
    let pc: RTCPeerConnection | null = null;

    onMount(() => {
        // canvas 尺寸必须在 mount 后设置，QRCodeStream 构造时从 canvas 属性读取
        canvasRef!.width = 320;
        canvasRef!.height = 320;
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
        // biome-ignore lint/suspicious/noExplicitAny: whip-whep.js uses any
        whipClient.onAnswer = (answer: any) =>
            convertSessionDescription(answer, "", "");

        setState("publishing");
        try {
            const url = `${location.origin}/whip/${searchParams.id || "-"}`;
            await whipClient.publish(pc, url, (searchParams.token as string) || "");
        } catch (e) {
            console.error(e);
            stopAll();
        }
    };

    const stopAll = async () => {
        if (whipClient) {
            await whipClient.stop();
            whipClient = null;
        }
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
                {/* canvas 隐藏，仅用于 QRCodeStream 渲染 */}
                <canvas ref={canvasRef} style="display: none;" />
                <section>
                    <video
                        ref={videoRef}
                        style={{ width: "320px" }}
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
