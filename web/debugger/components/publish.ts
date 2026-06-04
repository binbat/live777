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

export function getVideoSendEncodings(layer: string) {
    if (layer === "f") {
        return [{ scaleResolutionDownBy: 1.0 }];
    }

    const index = layers.findIndex((i) => i.rid === layer);
    if (index < 0) {
        return [{ scaleResolutionDownBy: 1.0 }];
    }

    return layers.slice(0 - (layers.length - index));
}

type startWhipConfig = {
    url: string;
    token: string;
    sourceMode?: "device" | "desktop" | "qrtime";
    inputStream?: MediaStream | null;
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
    const stream = cfg.inputStream
        ? cfg.inputStream
        : cfg.sourceMode === "desktop"
          ? await navigator.mediaDevices.getDisplayMedia({
                audio: true,
                video: videoSize,
            })
          : await navigator.mediaDevices.getUserMedia({
                audio: cfg.audio.device ? { deviceId: cfg.audio.device } : true,
                video: cfg.video.device
                    ? { deviceId: cfg.video.device, ...videoSize }
                    : Object.keys(videoSize).length > 0
                      ? videoSize
                      : true,
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

    const videoTransceiverInit: RTCRtpTransceiverInit = {
        direction: "sendonly",
    };
    const sendEncodings = getVideoSendEncodings(cfg.video.layer);
    videoTransceiverInit.sendEncodings = sendEncodings;
    const videoTransceiver = pc.addTransceiver(
        stream.getVideoTracks()[0],
        videoTransceiverInit,
    );
    const videoSenderParams = videoTransceiver.sender.getParameters();
    if (videoSenderParams.encodings?.[0]) {
        videoSenderParams.encodings[0].scaleResolutionDownBy = 1.0;
        (
            videoSenderParams as RTCRtpSendParameters & {
                degradationPreference?: string;
            }
        ).degradationPreference = "maintain-resolution";
        await videoTransceiver.sender.setParameters(videoSenderParams);
    }

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
    whip.onAnswer = (answer: RTCSessionDescriptionInit) =>
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
