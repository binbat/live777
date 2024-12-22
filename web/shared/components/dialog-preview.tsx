import { useRef, useImperativeHandle, useState, useContext } from 'preact/hooks';
import { TargetedEvent, forwardRef } from 'preact/compat';
import { Button, Collapse, Modal } from 'react-daisyui';
import { ClockIcon } from '@heroicons/react/24/outline';
import { WHEPClient } from '@binbat/whip-whep/whep.js';

import { TokenContext } from '../context';
import { formatVideoTrackResolution } from '../utils';
import { useLogger } from '../hooks/use-logger';
import { QRCodeStreamDecoder } from '../qrcode-stream';

interface Props {
    getWhepUrl?: (streamId: string) => string;
    onStop(): void
}

export interface IPreviewDialog {
    show(streamId: string): void
}

export const PreviewDialog = forwardRef<IPreviewDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState('');
    const tokenContext = useContext(TokenContext);
    const refPeerConnection = useRef<RTCPeerConnection | null>(null);
    const refWhepClient = useRef<WHEPClient | null>(null);
    const refMediaStream = useRef<MediaStream | null>(null);
    const [connState, setConnState] = useState('');
    const [videoResolution, setVideoResolution] = useState('');
    const refVideoResolutionInterval = useRef(-1);
    const logger = useLogger();
    const refDialog = useRef<HTMLDialogElement>(null);
    const refVideo = useRef<HTMLVideoElement>(null);
    const refStreamDecoder = useRef<QRCodeStreamDecoder>(null);
    const [latency, setLatency] = useState<string>();

    useImperativeHandle(ref, () => {
        return {
            show: async (newStreamId: string) => {
                if (streamId !== newStreamId) {
                    if (streamId !== '' && refWhepClient.current !== null) {
                        await handlePreviewStop();
                    }
                    setStreamId(newStreamId);
                    handlePreviewStart(newStreamId);
                }
                refDialog.current?.showModal();
            }
        };
    });

    const handleCloseDialog = () => {
        refDialog.current?.close();
    };

    const handlePreviewStop = async () => {
        window.clearInterval(refVideoResolutionInterval.current);
        if (refStreamDecoder.current) {
            refStreamDecoder.current.stop();
            refStreamDecoder.current = null;
        }
        if (refVideo.current) {
            refVideo.current.srcObject = null;
        }
        if (refMediaStream.current) {
            refMediaStream.current = null;
        }
        if (refPeerConnection.current) {
            refPeerConnection.current = null;
        }
        if (refWhepClient.current) {
            await refWhepClient.current.stop();
            refWhepClient.current = null;
        }
        props.onStop();
        handleCloseDialog();
    };

    const updateConnState = (state: string) => {
        setConnState(state);
        logger.log(state);
    };

    const logInboundRtpStats = async () => {
        const stats = await refPeerConnection.current?.getStats() ?? null;
        if (!stats) return;
        for (const [_, s] of stats) {
            if (s.type === 'inbound-rtp') {
                const { id, bytesReceived } = s as RTCInboundRtpStreamStats;
                // log the first time bytesReceived is not 0
                if (bytesReceived) {
                    logger.log(`inbound-rtp(${id}): ${bytesReceived} bytes`);
                    return;
                }
            }
        }
        window.queueMicrotask(logInboundRtpStats);
    };

    const handlePreviewStart = async (streamId: string) => {
        logger.clear();
        updateConnState('Started');
        const pc = new RTCPeerConnection();
        pc.addTransceiver('video', { direction: 'recvonly' });
        pc.addTransceiver('audio', { direction: 'recvonly' });
        const ms = new MediaStream();
        refMediaStream.current = ms;
        if (refVideo.current) {
            refVideo.current.srcObject = ms;
        }
        pc.addEventListener('track', ev => {
            logger.log(`track: ${ev.track.kind}`);
            ms.addTrack(ev.track);
        });
        pc.addEventListener('iceconnectionstatechange', () => {
            const state = pc.iceConnectionState;
            updateConnState(state);
            if (state === 'connected') {
                window.queueMicrotask(logInboundRtpStats);
            }
        });
        refPeerConnection.current = pc;
        const whep = new WHEPClient();
        whep.onOffer = sdp => {
            logger.log('http offer sent');
            return sdp;
        };
        whep.onAnswer = sdp => {
            logger.log('http answer received');
            return sdp;
        };
        refWhepClient.current = whep;
        try {
            const url = props.getWhepUrl?.(streamId) ?? `${location.origin}/whep/${streamId}`;
            await whep.view(pc, url, tokenContext.token);
        } catch (e: any) { // eslint-disable-line @typescript-eslint/no-explicit-any
            setConnState('Error');
            if (e instanceof Error) {
                logger.log(e.message);
            }
            const r = e.response as Response | undefined;
            if (r) {
                logger.log(await r.text());
            }
        }
        if (refVideoResolutionInterval.current >= 0) {
            window.clearInterval(refVideoResolutionInterval.current);
            refVideoResolutionInterval.current = -1;
        }
        refVideoResolutionInterval.current = window.setInterval(refreshVideoResolution, 1000);
    };

    const handleVideoCanPlay = (_: TargetedEvent<HTMLVideoElement>) => {
        logger.log('video canplay');
    };

    const refreshVideoResolution = (_: TargetedEvent<HTMLVideoElement>) => {
        if (refMediaStream.current) {
            const videoTrack = refMediaStream.current.getVideoTracks()[0];
            if (videoTrack) {
                setVideoResolution(formatVideoTrackResolution(videoTrack));
            }
        }
    };

    const handleDecodeLatency = (e: TargetedEvent) => {
        e.preventDefault();
        setLatency('-- ms');
        if (refVideo.current != null && refStreamDecoder.current == null) {
            refStreamDecoder.current = new QRCodeStreamDecoder(refVideo.current);
        }
        const decoder = refStreamDecoder.current!;
        decoder.start();
        decoder.addEventListener('latency', (e: CustomEvent<number>) => {
            setLatency(`${e.detail} ms`);
        });
    };

    return (
        <Modal ref={refDialog} className="min-w-md max-w-[unset] w-[unset]">
            <Modal.Header className="mb-6">
                <h3 className="font-bold">Preview {streamId} {videoResolution}</h3>
            </Modal.Header>
            <Modal.Body>
                <video
                    ref={refVideo}
                    className="mx-[-1.5rem] min-w-[28rem] max-w-[90vw] max-h-[70vh]"
                    onCanPlay={handleVideoCanPlay}
                    onResize={refreshVideoResolution}
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
                <div className="mr-auto">
                    {typeof latency === 'string' ? (
                        <Button
                            color="ghost"
                            className="font-normal"
                            animation={false}
                            startIcon={<ClockIcon className="size-5 stroke-current" />}
                        >Latency: {latency}</Button>
                    ) : (
                        <Button color="info" onClick={handleDecodeLatency}>Decode Latency</Button>
                    )}
                </div>
                <Button color="error" onClick={handlePreviewStop}>Stop</Button>
                <Button onClick={handleCloseDialog}>Hide</Button>
            </Modal.Actions>
        </Modal>
    );
});
