import { useRef, useImperativeHandle, useState, useContext } from 'preact/hooks';
import { TargetedEvent, forwardRef } from 'preact/compat';
import { Button, Collapse, Modal } from 'react-daisyui';
import { WHIPClient } from '@binbat/whip-whep/whip';

import { TokenContext } from '../context';
import { formatVideoTrackResolution } from '../utils';
import { useLogger } from '../hooks/use-logger';
import { QRCodeStream } from '../qrcode-stream';

interface Props {
    getWhipUrl?: (streamId: string) => string;
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
    const refCanvas = useRef<HTMLCanvasElement>(null);
    const refQrCodeStream = useRef<QRCodeStream>(null);

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

    const handleStreamStart = async (stream: MediaStream) => {
        logger.clear();
        setConnState('');
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
        whip.onOffer = sdp => {
            logger.log('http offer sent');
            return sdp;
        };
        whip.onAnswer = sdp => {
            logger.log('http answer received');
            return sdp;
        };
        try {
            const url = props.getWhipUrl?.(streamId) ?? `${location.origin}/whep/${streamId}`;
            await whip.publish(pc, url, tokenContext.token);
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

    const handleDisplayMediaStart = async () => {
        const stream = await navigator.mediaDevices.getDisplayMedia({
            audio: true,
            video: true
        });
        handleStreamStart(stream);
    };

    const handleEncodeLatencyStart = () => {
        if (!refQrCodeStream.current) {
            refQrCodeStream.current = new QRCodeStream(refCanvas.current!);
        }
        handleStreamStart(refQrCodeStream.current!.capture());
    };

    const handleStreamStop = async () => {
        if (refQrCodeStream.current) {
            refQrCodeStream.current.stop();
            refQrCodeStream.current = null;
        }
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
        <Modal ref={refDialog} className="min-w-md max-w-[unset] w-[unset]">
            <Modal.Header className="mb-6">
                <h3 className="font-bold">Web Stream {streamId} {videoResolution}</h3>
            </Modal.Header>
            <Modal.Body>
                <video
                    ref={refVideo}
                    className="mx-[-1.5rem] min-w-[28rem] max-w-[90vw] max-h-[70vh]"
                    onResize={handleVideoResize}
                    controls autoplay
                />
                <Collapse.Details icon="arrow" className="text-sm">
                    <Collapse.Details.Title className="px-0">
                        <b>Connection Status: </b>
                        <code>{connState}</code>
                    </Collapse.Details.Title>
                    <Collapse.Content className="px-0">
                        <pre class="overflow-auto max-h-[10lh]">{logger.logs.join('\n')}</pre>
                    </Collapse.Content>
                </Collapse.Details>
            </Modal.Body>
            <Modal.Actions className="mt-0">
                {refWhipClient.current ? (
                    <Button color="error" onClick={handleStreamStop}>Stop</Button>
                ) : (
                    <>
                        <Button color="info" onClick={handleEncodeLatencyStart}>Encode Latency</Button>
                        <Button onClick={handleDisplayMediaStart}>Start</Button>
                    </>
                )}
                <Button onClick={handleCloseDialog}>Hide</Button>
            </Modal.Actions>
            <canvas ref={refCanvas} class="hidden" width={1280} height={720}></canvas>
        </Modal>
    );
});
