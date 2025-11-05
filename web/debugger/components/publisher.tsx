import { useSearchParams } from "@solidjs/router";
import { createSignal, Show } from "solid-js";
import { createLogger } from "../primitive/logger";
import Datachannel from "./datachannel";
import Device from "./device";
import Player from "./player";
import publish from "./publish";

const AudioCodecOptions = [
    { value: "", text: "default" },
    { value: "opus", text: "OPUS" },
    { value: "g722", text: "G722" },
];

const VideoCodecOptions = [
    { value: "", text: "default" },
    { value: "av1", text: "AV1" },
    { value: "vp9", text: "VP9" },
    { value: "vp8", text: "VP8" },
    { value: "h264", text: "H264" },
];

const mapCodec: Record<string, string> = {
    "": "",
    av1: "av1/90000",
    vp9: "vp9/90000",
    vp8: "vp8/90000",
    h264: "h264/90000",
    opus: "opus/48000",
    g722: "g722/8000",
};

const VideoWidthOptions = [
    { value: "", text: "default" },
    { value: "320", text: "320px" },
    { value: "480", text: "480px" },
    { value: "600", text: "600px" },
    { value: "1280", text: "1280px" },
    { value: "1920", text: "1920px" },
    { value: "3480", text: "3480px" },
];

const VideoHeightOptions = [
    { value: "", text: "default" },
    { value: "240", text: "240px" },
    { value: "320", text: "320px" },
    { value: "480", text: "480px" },
    { value: "720", text: "720px" },
    { value: "1080", text: "1080px" },
    { value: "2160", text: "2160px" },
];

const WhipLayerOptions = [
    { value: "f", text: "Base" },
    { value: "h", text: "Base + 1/2" },
    { value: "q", text: "Base + 1/2 + 1/4" },
];

export default function Publisher() {
    const [disabled, setDisabled] = createSignal(false);
    const [stream, setStream] = createSignal<MediaStream | null>(null);
    const [datachannel, setDatachannel] = createSignal<RTCDataChannel | null>(
        null,
    );
    const [selectAudioDevice, setSelectAudioDevice] = createSignal("");
    const [selectVideoDevice, setSelectVideoDevice] = createSignal("");
    const [selectVideoWidth, setSelectVideoWidth] = createSignal("");
    const [selectVideoHeight, setSelectVideoHeight] = createSignal("");
    const [selectAudioPseudo, setSelectAudioPseudo] = createSignal(false);
    const [selectVideoLayer, setSelectVideoLayer] = createSignal("f");

    const [audioTrackCount, setAudioTrackCount] = createSignal(0);
    const [videoTrackCount, setVideoTrackCount] = createSignal(0);

    const [searchParams, setSearchParams] = useSearchParams();

    const [logs, setLogs, clear] = createLogger();

    let stop: () => Promise<void> | undefined;

    const start = async () => {
        setDisabled(true);
        clear();
        stop = await publish({
            url: `${location.origin}/whip/${searchParams.id || "-"}`,
            token: (searchParams.token as string) || "",
            audio: {
                device: selectAudioDevice(),
                codec: mapCodec[(searchParams.acodec as string) || ""],
                pseudo: selectAudioPseudo(),
            },
            video: {
                device: selectVideoDevice(),
                codec: mapCodec[(searchParams.vcodec as string) || ""],
                layer: selectVideoLayer(),
                width: parseInt(selectVideoWidth(), 10) || null,
                height: parseInt(selectVideoHeight(), 10) || null,
            },
            onStream: (stream: MediaStream | null): void => {
                setAudioTrackCount(stream ? stream.getAudioTracks().length : 0);
                setVideoTrackCount(stream ? stream.getVideoTracks().length : 0);
                setStream(stream);
            },
            onChannel: (channel: RTCDataChannel): void => {
                setDatachannel(channel);
            },
            log: setLogs,
        });
    };
    return (
        <>
            <legend>WHIP</legend>
            <div style="text-align: center;">
                <section>
                    <Device
                        disabled={disabled()}
                        onSelectAudio={(deviceId) =>
                            setSelectAudioDevice(deviceId)
                        }
                        onSelectVideo={(deviceId) =>
                            setSelectVideoDevice(deviceId)
                        }
                    />
                </section>

                <section>
                    <label>
                        Audio Codec:
                        <select
                            onChange={(e) => {
                                setSearchParams({ acodec: e.target.value });
                            }}
                            disabled={disabled()}
                            value={(searchParams.acodec as string) || ""}
                        >
                            {AudioCodecOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        Video Codec:
                        <select
                            onChange={(e) => {
                                setSearchParams({ vcodec: e.target.value });
                            }}
                            disabled={disabled()}
                            value={(searchParams.vcodec as string) || ""}
                        >
                            {VideoCodecOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                </section>
                <section>
                    <label>
                        Video Width:
                        <select
                            onChange={(e) => {
                                setSelectVideoWidth(e.target.value);
                            }}
                            disabled={disabled()}
                        >
                            {VideoWidthOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                    <label>
                        Video Height:
                        <select
                            disabled={disabled()}
                            onChange={(e) => {
                                setSelectVideoHeight(e.target.value);
                            }}
                        >
                            {VideoHeightOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                </section>
                <section>
                    <input
                        type="checkbox"
                        checked={selectAudioPseudo()}
                        onChange={(e) => {
                            setSelectAudioPseudo(e.target.checked);
                        }}
                        disabled={disabled()}
                    />
                    Use Pseudo Audio Track
                </section>
                <section>
                    <label>
                        SVC Layer:
                        <select
                            onChange={(e) => {
                                setSelectVideoLayer(e.target.value);
                            }}
                            disabled={disabled()}
                        >
                            {WhipLayerOptions.map((o) => (
                                <option value={o.value}>{o.text}</option>
                            ))}
                        </select>
                    </label>
                </section>
                <section>
                    <button type="button" onClick={start} disabled={disabled()}>
                        Start
                    </button>
                    <button
                        type="button"
                        onClick={() => {
                            setDisabled(false);
                            stop();
                        }}
                        disabled={!disabled()}
                    >
                        Stop
                    </button>
                </section>
                <section>
                    <h3>WHIP Video:</h3>
                    <h5>
                        Audio Track Count: {audioTrackCount()}, Video Track
                        Count: {videoTrackCount()}
                    </h5>
                    <Show when={stream()}>
                        {(ms) => <Player stream={ms()} />}
                    </Show>
                </section>
                <section>
                    <Show when={datachannel()}>
                        {(dc) => <Datachannel datachannel={dc()} />}
                    </Show>
                </section>
                <section>
                    <h4>Logs:</h4>
                    <pre>{logs().join("\n")}</pre>
                </section>
            </div>
        </>
    );
}
