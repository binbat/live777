import { TargetedEvent } from 'preact/compat';
import { useEffect, useState } from 'preact/hooks';

declare module 'preact' {
    // preact uses namespace JSXInternal and exports it as JSX
    // eslint-disable-next-line @typescript-eslint/no-namespace
    namespace JSX {
        interface IntrinsicElements {
            'center': JSX.HTMLAttributes<HTMLElement>
            'data-channel': JSX.HTMLAttributes<HTMLElement>;
            'debug-player': JSX.HTMLAttributes<HTMLElement>;
            'video-size-select': JSX.HTMLAttributes<HTMLElement>;
        }
    }
}

declare global {
    interface Window {
        startWhip(): Promise<void>;
        startWhep(): Promise<void>;
    }
}

function useUrlParamsInput(key: string) {
    const [value, setValue] = useState('');
    useEffect(() => {
        const params = new URLSearchParams(location.search);
        const v = params.get(key);
        if (v !== null) {
            setValue(v);
        }
    }, []);
    const setUrlParams = (v: string | undefined) => {
        const params = new URLSearchParams(location.search);
        if (v === undefined) {
            params.delete(key);
        } else {
            params.set(key, v);
        }
        history.replaceState(null, '', '?' + params.toString());
    };
    const onInput = (e: TargetedEvent<HTMLInputElement>) => {
        const v = e.currentTarget.value;
        setValue(v);
        setUrlParams(v);
    };
    return { value, onInput };
}

const AudioCodecOptions = [
    { value: '', text: 'default' },
    { value: 'opus/48000', text: 'OPUS' },
    { value: 'g722/8000', text: 'G722' },
];
const VideoCodecOptions = [
    { value: '', text: 'default' },
    { value: 'av1/90000', text: 'AV1' },
    { value: 'vp9/90000', text: 'VP9' },
    { value: 'vp8/90000', text: 'VP8' },
    { value: 'h264/90000', text: 'H264' },
];
const WhipLayerOptions = [
    { value: 'f', text: 'Base' },
    { value: 'h', text: 'Base + 1/2' },
    { value: 'q', text: 'Base + 1/2 + 1/4' },
];
const WhepLayerOptions = [
    { value: '', text: 'AUTO' },
    { value: 'q', text: 'LOW' },
    { value: 'h', text: 'MEDIUM' },
    { value: 'f', text: 'HIGH' },
];

const NoneDevice = { value: '', text: 'none' };

function deviceInfoToOption(info: MediaDeviceInfo) {
    const value = info.deviceId;
    let text = info.label;
    if (text.length <= 0) {
        text = `${info.kind} (${info.deviceId})`;
    }
    return { value, text };
}

function uniqByValue<T extends { value: unknown }>(items: T[]) {
    const map = new Map<unknown, T>();
    for (const item of items) {
        if (!map.has(item.value)) {
            map.set(item.value, item);
        }
    }
    return Array.from(map.values());
}

export default function DebuggerCompat() {
    const streamIdInput = useUrlParamsInput('id');
    const idBearerTokenInput = useUrlParamsInput('token');

    const [refreshDisabled, setRefreshDisabled] = useState(false);
    const [audioDevices, setAudioDevices] = useState([NoneDevice]);
    const [videoDevices, setVideoDevices] = useState([NoneDevice]);

    const refreshDevice = async () => {
        try {
            // to obtain non-empty device label, there needs to be an active media stream or persistent permission
            // https://developer.mozilla.org/en-US/docs/Web/API/MediaDeviceInfo/label#value
            const mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: true });
            const devices = (await navigator.mediaDevices.enumerateDevices()).filter(i => !!i.deviceId);
            mediaStream.getTracks().map(track => track.stop());
            const audio = devices.filter(i => i.kind === 'audioinput').map(deviceInfoToOption);
            if (audio.length > 0) {
                setAudioDevices(uniqByValue(audio));
            }
            const video = devices.filter(i => i.kind === 'videoinput').map(deviceInfoToOption);
            if (video.length > 0) {
                setVideoDevices(uniqByValue(video));
            }
        } catch (e) {
            console.error('refreshDevice failed:', e);
        }
        setRefreshDisabled(true);
    };

    return (
        <>
            <fieldset>
                <legend>Common</legend>
                <section style="display: flex;justify-content: space-evenly;flex-wrap: wrap;">
                    <div>Stream ID: <input id="id" type="text" {...streamIdInput} /></div>
                    <div>Bearer Token: <input id="token" type="text" {...idBearerTokenInput} /></div>
                </section>
            </fieldset>

            <div style="display: flex;justify-content: space-evenly;flex-wrap: wrap;">
                <fieldset>
                    <legend>WHIP</legend>
                    <center>
                        <section>
                            <button id="whip-device-button" disabled={refreshDisabled} onClick={refreshDevice}>Use Device</button>
                            <div style="margin: 0.2rem">Audio Device:
                                <select id="whip-audio-device">
                                    {audioDevices.map(d => <option value={d.value}>{d.text}</option>)}
                                </select>
                            </div>
                            <div style="margin: 0.2rem">Video Device:
                                <select id="whip-video-device">
                                    {videoDevices.map(d => <option value={d.value}>{d.text}</option>)}
                                </select>
                            </div>
                        </section>

                        <section>
                            Audio Codec: <select id="whip-audio-codec">
                                {AudioCodecOptions.map(o => <option value={o.value}>{o.text}</option>)}
                            </select>
                            Video Codec: <select id="whip-video-codec">
                                {VideoCodecOptions.map(o => <option value={o.value}>{o.text}</option>)}
                            </select>
                        </section>
                        <section>
                            <video-size-select id="whip-video-size"></video-size-select>
                        </section>
                        <section>SVC Layer: <select id="whip-layer-select">
                            {WhipLayerOptions.map(o => <option value={o.value}>{o.text}</option>)}
                        </select>
                        </section>
                        <section>
                            <input type="checkbox" id="whip-pseudo-audio" />Pseudo Audio Track
                        </section>
                        <section>
                            <button onClick={window.startWhip}>Start</button>
                            <button id="whip-button-stop">Stop</button>
                        </section>

                        <section>
                            <h3>WHIP Video:</h3>
                            <debug-player id="whip-video-player"></debug-player>
                        </section>
                        <section>
                            <data-channel id="whip-datachannel"></data-channel>
                        </section>
                        <br />Logs: <br />
                        <div id="whip-logs"></div>
                    </center>
                </fieldset>

                <fieldset>
                    <legend>WHEP</legend>
                    <center>
                        <section>SVC Layer: <select disabled id="whep-layer-select">
                            {WhepLayerOptions.map(o => <option value={o.value}>{o.text}</option>)}
                        </select>
                        </section>
                        <section>
                            <button id="whep-button-disable-audio">Disable Audio</button>
                            <button id="whep-button-disable-video">Disable Video</button>
                        </section>
                        <section>
                            <button onClick={window.startWhep}>Start</button>
                            <button id="whep-button-stop">Stop</button>
                        </section>

                        <section>
                            <h3>WHEP Video:</h3>
                            <debug-player id="whep-video-player"></debug-player>
                        </section>
                        <section>
                            <data-channel id="whep-datachannel"></data-channel>
                        </section>
                        <br />Logs: <br />
                        <div id="whep-logs"></div>
                    </center>
                </fieldset>
            </div>
        </>
    );
}
