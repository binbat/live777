import { useRef, useImperativeHandle, useState, useContext } from 'preact/hooks';
import { TargetedEvent, forwardRef } from 'preact/compat';
import { WHIPClient } from '@binbat/whip-whep/whip';

import { TokenContext } from '../context';
import { formatVideoTrackResolution } from '../utils';
import { useLogger } from '../hooks/use-logger';

interface Props {
    onStop(): void
}

export interface IWebStreamDialog {
    show(streamId: string): void
}

export const WebStreamDialog = forwardRef<IWebStreamDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState('');
    const tokenContext = useContext(TokenContext);
    const refMediaStream = useRef<MediaStream | null>(null);
    const refWhipClient = useRef<WHIPClient | null>(null);
    const [connState, setConnState] = useState('');
    const [videoResolution, setVideoResolution] = useState('');
    const logger = useLogger();
    const refDialog = useRef<HTMLDialogElement>(null);
    const refVideo = useRef<HTMLVideoElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setStreamId(streamId);
                refDialog.current?.showModal();
            }
        };
    });

    const handleCloseDialog = () => {
        refDialog.current?.close();
    };

    const updateConnState = (state: string) => {
        setConnState(state);
        logger.log(state);
    };

    const handleStreamStart = async () => {
        logger.clear();
        setConnState('');
        const stream = await navigator.mediaDevices.getDisplayMedia({
            audio: true,
            video: true
        });
        refMediaStream.current = stream;
        if (refVideo.current) {
            refVideo.current.srcObject = stream;
        }
        updateConnState('Started');
        const pc = new RTCPeerConnection();
        pc.addEventListener('iceconnectionstatechange', () => {
            updateConnState(pc.iceConnectionState);
        });
        stream.getVideoTracks().forEach(vt => {
            pc.addTransceiver(vt, { direction: 'sendonly' });
            setVideoResolution(formatVideoTrackResolution(vt));
        });
        stream.getAudioTracks().forEach(at => {
            pc.addTransceiver(at, { direction: 'sendonly' });
        });
        const whip = new WHIPClient();
        refWhipClient.current = whip;
        const url = `${location.origin}/whip/${streamId}`;
        whip.onOffer = sdp => {
            logger.log('http offer sent');
            return sdp;
        };
        whip.onAnswer = sdp => {
            logger.log('http answer received');
            return sdp;
        };
        try {
            await whip.publish(pc, url, tokenContext?.token ?? '');
        } catch (e: any) {  // eslint-disable-line @typescript-eslint/no-explicit-any
            setConnState('Error');
            if (e instanceof Error) {
                logger.log(e.message);
            }
            const r = e.response as Response | undefined;
            if (r) {
                logger.log(await r.text());
            }
        }
    };

    const handleStreamStop = async () => {
        if (refMediaStream.current) {
            refMediaStream.current.getTracks().forEach(t => t.stop());
            refMediaStream.current = null;
        }
        if (refVideo.current) {
            refVideo.current.srcObject = null;
        }
        if (refWhipClient.current) {
            await refWhipClient.current.stop();
            refWhipClient.current = null;
        }
        props.onStop();
        handleCloseDialog();
    };

    const handleVideoResize = (_: TargetedEvent<HTMLVideoElement>) => {
        const videoTrack = refMediaStream.current?.getVideoTracks()[0];
        if (videoTrack) {
            setVideoResolution(formatVideoTrackResolution(videoTrack));
        }
    };

    return (
        <dialog ref={refDialog}>
            <h3>Web Stream {streamId} {videoResolution}</h3>
            <div>
                <video ref={refVideo} controls autoplay onResize={handleVideoResize} class="max-w-[90vw] max-h-[70vh]"></video>
            </div>
            <details>
                <summary>
                    <b>Connection Status: </b>
                    <code>{connState}</code>
                </summary>
                <pre class="overflow-auto max-h-[10lh]">{logger.logs.join('\n')}</pre>
            </details>
            <div>
                <button onClick={() => { handleCloseDialog(); }}>Hide</button>
                {refWhipClient.current
                    ? <button onClick={() => { handleStreamStop(); }} class="text-red-500">Stop</button>
                    : <button onClick={() => { handleStreamStart(); }}>Start</button>
                }
            </div>
        </dialog>
    );
});
