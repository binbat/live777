import { useState, useRef, useEffect } from 'preact/hooks';
import { TargetedEvent } from 'preact/compat';

import { WHEPClient } from '@binbat/whip-whep/whep.js';

export function Player() {
    const [streamId, setStreamId] = useState('');
    const [autoPlay, setAutoPlay] = useState(false);
    const [muted, setMuted] = useState(false);
    const [controls, setControls] = useState(false);
    const [reconnect, setReconnect] = useState(0);
    const [token, setToken] = useState('');
    const refPeerConnection = useRef<RTCPeerConnection | null>(null);
    const refWhepClient = useRef<WHEPClient | null>(null);
    const refMediaStream = useRef<MediaStream | null>(null);
    const refVideo = useRef<HTMLVideoElement>(null);

    useEffect(() => {
        const params = new URLSearchParams(location.search);
        setStreamId(params.get('id') ?? '');
        setAutoPlay(params.has('autoplay'));
        setControls(params.has('controls'));
        setMuted(params.has('muted'));
        const n = Number.parseInt(params.get('reconnect') ?? '0', 10);
        setReconnect(Number.isNaN(n) ? 0 : n);
        setToken(params.get('token') ?? '');
    }, []);

    useEffect(() => {
        if (!streamId || !autoPlay) return;
        handlePlay();
    }, [streamId, autoPlay]);

    const handlePlay = async () => {
        const pc = new RTCPeerConnection();
        refPeerConnection.current = pc;
        pc.addTransceiver('video', { direction: 'recvonly' });
        pc.addTransceiver('audio', { direction: 'recvonly' });
        const ms = new MediaStream();
        refMediaStream.current = ms;
        if (refVideo.current) {
            refVideo.current.srcObject = ms;
        }
        pc.addEventListener('track', ev => {
            ms.addTrack(ev.track);
        });
        pc.addEventListener('iceconnectionstatechange', () => {
            switch (pc.iceConnectionState) {
                case 'disconnected': {
                    handleStop();
                    break;
                }
            }
        });
        const whep = new WHEPClient();
        refWhepClient.current = whep;
        const url = `${location.origin}/whep/${streamId}`;
        try {
            await whep.view(pc, url, token);
        } catch {
            handleStop();
        }
    };

    const handleStop = async () => {
        if (refVideo.current) {
            refVideo.current.srcObject = null;
        }
        if (refWhepClient.current) {
            await refWhepClient.current.stop();
            refWhepClient.current = null;
        }
        if (refPeerConnection.current) {
            refPeerConnection.current = null;
        }
        if (reconnect > 0) {
            setTimeout(() => { handleReconnect(); }, reconnect);
        }
    };

    const handleReconnect = async () => {
        await handlePlay();
        refVideo.current?.play();
    };

    const handleVideoClick = async (e: TargetedEvent<HTMLVideoElement>) => {
        if (!refWhepClient.current) {
            e.preventDefault();
            await handlePlay();
            refVideo.current?.play();
        }
    };

    return (
        <div id="player">
            <video
                ref={refVideo}
                autoplay={autoPlay}
                muted={muted}
                controls={controls}
                onClick={e => handleVideoClick(e)}
            />
        </div>
    );
}
