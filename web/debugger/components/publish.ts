import { WHIPClient } from "@binbat/whip-whep/whip.js";
import convertSessionDescription from "./sdp";

// NOTE:
// https://github.com/livekit/client-sdk-js/blob/761711adb4195dc49a0b32e1e4f88726659dac94/src/room/track/LocalVideoTrack.ts#L492
// - f: HIGH
// - h: MEDIUM
// - q: LOW
const layers = [
    { rid: "q", scaleResolutionDownBy: 4.0, scalabilityMode: "L1T3" },
    { rid: "h", scaleResolutionDownBy: 2.0, scalabilityMode: "L1T3" },
    { rid: "f", scalabilityMode: "L1T3" },
];

type startWhipConfig = {
    url: string;
    token: string;
    audio: {
        device: string;
        codec: string;
        pseudo: boolean;
    };
    video: {
        device: string;
        codec: string;
        layer: string;
        width: number | null;
        height: number | null;
    };
    onStream: (stream: MediaStream | null) => void;
    onChannel: (channel: RTCDataChannel) => void;
    log: (msg: string) => void;
};

export default async function startWhip(
    cfg: startWhipConfig,
): Promise<() => Promise<void>> {
    cfg.log("started");
    const videoSize = {
        width: cfg.video.width,
        height: cfg.video.height,
    } as MediaTrackConstraints;
    cfg.log(
        `video width: ${!videoSize.width ? "default" : videoSize.width}, height: ${!videoSize.height ? "default" : videoSize.height}`,
    );
    cfg.log(`audio device: ${!cfg.audio.device ? "none" : cfg.audio.device}`);
    cfg.log(`video device: ${!cfg.video.device ? "none" : cfg.video.device}`);

    const stream =
        !cfg.audio.device && !cfg.video.device
            ? await navigator.mediaDevices.getDisplayMedia({
                  audio: true,
                  video: videoSize,
              })
            : await navigator.mediaDevices.getUserMedia({
                  audio: { deviceId: cfg.audio.device },
                  video: { deviceId: cfg.video.device, ...videoSize },
              });

    cfg.onStream(stream);

    const pc = new RTCPeerConnection();

    // NOTE:
    // 1. Live777 Don't support label
    // 2. Live777 Don't support negotiated
    cfg.onChannel(pc.createDataChannel(""));

    pc.oniceconnectionstatechange = () =>
        cfg.log(`ICE State: ${pc.iceConnectionState}`);
    pc.onconnectionstatechange = () =>
        cfg.log(`connection State: ${pc.connectionState}`);

    const index = layers.findIndex((i) => i.rid === cfg.video.layer);

    const sendEncodings = layers.slice(0 - (layers.length - index));
    pc.addTransceiver(stream.getVideoTracks()[0], {
        direction: "sendonly",
        sendEncodings: sendEncodings,
    });

    if (cfg.audio.pseudo) {
        pc.addTransceiver("audio", { direction: "sendonly" });
    } else {
        stream.getAudioTracks().forEach((track) => {
            pc.addTransceiver(track, {
                direction: "sendonly",
            });
        });
    }

    cfg.log(`audio codec: ${!cfg.audio.codec ? "default" : cfg.audio.codec}`);
    cfg.log(`video codec: ${!cfg.video.codec ? "default" : cfg.video.codec}`);

    const whip = new WHIPClient();
    // biome-ignore lint/suspicious/noExplicitAny: This whip-whep.js use any type
    whip.onAnswer = (answer: any) =>
        convertSessionDescription(answer, cfg.audio.codec, cfg.video.codec);

    try {
        cfg.log("http begined");
        await whip.publish(pc, cfg.url, cfg.token);
    } catch (e) {
        cfg.log(`ERROR: ${e}`);
    }

    const stop = async () => {
        await whip.stop();
        cfg.log("stopped");
        stream.getTracks().map((track) => track.stop());
        cfg.onStream(null);
    };
    return stop;
}
