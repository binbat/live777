import { WHEPClient } from "@binbat/whip-whep/whep.js";

type startWhepConfig = {
    url: string;
    token: string;
    onStream: (stream: MediaStream | null) => void;
    onChannel: (channel: RTCDataChannel) => void;
    log: (msg: string) => void;
};

export default async function startWhep(cfg: startWhepConfig): Promise<
    [
        () => Promise<void>,
        // biome-ignore lint/suspicious/noExplicitAny: This whip-whep.js use any type
        (muted: any) => Promise<void>,
        (layer: string) => Promise<void>,
    ]
> {
    const is404Error = (e: unknown) => {
        const maybe = e as { response?: { status?: number }; status?: number };
        const status = maybe?.response?.status ?? maybe?.status;
        return status === 404 || String(e).includes("404");
    };

    cfg.log("started");
    const pc = new RTCPeerConnection();

    // NOTE:
    // 1. Live777 Don't support label
    // 2. Live777 Don't support negotiated
    cfg.onChannel(pc.createDataChannel(""));

    pc.oniceconnectionstatechange = () =>
        cfg.log(`ICE State: ${pc.iceConnectionState}`);
    pc.onconnectionstatechange = () =>
        cfg.log(`connection State: ${pc.connectionState}`);
    pc.addTransceiver("video", { direction: "recvonly" });
    pc.addTransceiver("audio", { direction: "recvonly" });

    const ms = new MediaStream();
    pc.ontrack = (ev) => {
        cfg.log(`track: ${ev.track.kind}`);

        ms.addTrack(ev.track);
        // addtrack removetrack events won't fire when calling addTrack/removeTrack in javascript
        // https://github.com/w3c/mediacapture-main/issues/517
        cfg.onStream(ms);
    };
    const whep = new WHEPClient();

    try {
        cfg.log("http begined");
        await whep.view(pc, cfg.url, cfg.token);
    } catch (e) {
        cfg.log(`ERROR: ${e}`);
    }

    const stop = async () => {
        try {
            await whep.stop();
        } catch (e) {
            if (!is404Error(e)) {
                throw e;
            }
            cfg.log("stop ignored: session already closed (404)");
        }
        cfg.log("stopped");
        cfg.onStream(null);
    };

    // biome-ignore lint/suspicious/noExplicitAny: This whip-whep.js use any type
    const mute = async (muted: any) => {
        cfg.log(`mute: ${JSON.stringify(muted)}`);
        await whep.mute(muted);
    };

    const selectLayer = async (layer: string) => {
        !layer
            ? await whep.unselectLayer()
            : //@ts-expect-error
              await whep
                  .selectLayer({ encodingId: layer })
                  .catch((e) => cfg.log(e));
    };

    return [stop, mute, selectLayer];
}
