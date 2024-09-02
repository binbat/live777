import { TargetedEvent } from 'preact/compat';
import { useCallback, useEffect, useState } from 'preact/hooks';

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

const WhipLayerSelect = [
    { value: 'f', text: 'Base' },
    { value: 'h', text: 'Base + 1/2' },
    { value: 'q', text: 'Base + 1/2 + 1/4' },
];
const WhepLayerSelect = [
    { value: '', text: 'AUTO' },
    { value: 'q', text: 'LOW' },
    { value: 'h', text: 'MEDIUM' },
    { value: 'f', text: 'HIGH' },
];

const NoneDevice = { value: '', text: 'none' };

export default function DebuggerCompat() {
    const streamIdInput = useUrlParamsInput('id');
    const idBearerTokenInput = useUrlParamsInput('token');

    const [refreshDisabled, setRefreshDisabled] = useState(false);
    const [audioDevices, setAudioDevices] = useState([NoneDevice]);
    const [videoDevices, setVideoDevices] = useState([NoneDevice]);

    const refreshDevice = useCallback(async () => {
        try {
            const mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: true });
            mediaStream.getTracks().map(track => track.stop());
        } catch (e) {
            console.error('Failed to getUserMedia:', e);
        }
        const devices = (await navigator.mediaDevices.enumerateDevices()).filter(i => !!i.deviceId);
        const audio = devices.filter(i => i.kind === 'audioinput').map(i => ({ value: i.deviceId, text: i.label }));
        if (audio.length > 0) {
            setAudioDevices(audio);
        }
        const video = devices.filter(i => i.kind === 'videoinput').map(i => ({ value: i.deviceId, text: i.label }));
        if (video.length > 0) {
            setVideoDevices(video);
        }
        setRefreshDisabled(true);
    }, []);
    const startWhip = useCallback(() => window.startWhip(), []);
    const startWhep = useCallback(() => window.startWhep(), []);

    return (
        <>
            <fieldset>
                <legend>Common</legend>
                <section style="display: flex;justify-content: space-evenly;flex-wrap: wrap;">
                    <div>Stream ID: <input id="id" type="text" {...streamIdInput} /></div>
                    <div>Bearer Token: <input id="token" type="text"  {...idBearerTokenInput} /></div>
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
                                <option value="" selected>default</option>
                                <option value="opus/48000">OPUS</option>
                                <option value="g722/8000">G722</option>
                            </select>
                            Video Codec: <select id="whip-video-codec">
                                <option value="" selected>default</option>
                                <option value="av1/90000">AV1</option>
                                <option value="vp9/90000">VP9</option>
                                <option value="vp8/90000">VP8</option>
                                <option value="h264/90000">H264</option>
                            </select>
                        </section>
                        <section>
                            <video-size-select id="whip-video-size"></video-size-select>
                        </section>
                        <section>SVC Layer: <select id="whip-layer-select">
                            {WhipLayerSelect.map(l => <option value={l.value}>{l.text}</option>)}
                        </select>
                        </section>
                        <section>
                            <input type="checkbox" id="whip-pseudo-audio" />Pseudo Audio Track
                        </section>
                        <section>
                            <button onClick={startWhip}>Start</button>
                            <button id="whip-button-stop">Stop</button>
                        </section>

                        <section>
                            <h3>WHIP Video:</h3>
                            <debug-player controls autoplay id="whip-video-player"></debug-player>
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
                            {WhepLayerSelect.map(l => <option value={l.value}>{l.text}</option>)}
                        </select>
                        </section>
                        <section>
                            <button id="whep-button-disable-audio">Disable Audio</button>
                            <button id="whep-button-disable-video">Disable Video</button>
                        </section>
                        <section>
                            <button onClick={startWhep}>Start</button>
                            <button id="whep-button-stop">Stop</button>
                        </section>

                        <section>
                            <h3>WHEP Video:</h3>
                            <debug-player controls autoplay id="whep-video-player"></debug-player>
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
