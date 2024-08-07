import { useState, useRef, useEffect } from 'preact/hooks';
import { WHEPClient } from '@binbat/whip-whep/whep.js';

export function Player() {
    const [streamId, setStreamId] = useState('');
    const [autoPlay, setAutoPlay] = useState(false);
    const [muted, setMuted] = useState(false);
    const [controls, setControls] = useState(false);
    const [reconnect, setReconnect] = useState(0);
    const [peerConnection, setPeerConnection] = useState<RTCPeerConnection | null>(null);
    const [whepClient, setWhepClient] = useState<WHEPClient | null>(null);
    const refAudio = useRef<HTMLAudioElement>(document.createElement('audio'));
    const refVideo = useRef<HTMLVideoElement>(null);

    useEffect(() => {
        const params = new URLSearchParams(location.search);
        setStreamId(params.get('id') ?? '');
        setAutoPlay(params.has('autoplay'));
        setControls(params.has('controls'));
        setMuted(params.has('muted'));
        const n = Number.parseInt(params.get('reconnect') ?? '0', 10);
        setReconnect(Number.isNaN(n) ? 0 : n);
    });

    useEffect(() => {
        if (!streamId || !autoPlay) return;
        handlePlay();
        const v = refVideo.current;
        if (v) {
            v.volume = 0;
            v.play();
        }
    }, [streamId]);

    useEffect(() => {
        const v = refVideo.current;
        if (v) {
            v.muted = muted;
        }

        if (muted) {
            refAudio.current.pause();
        } else {
            refAudio.current.play();
        }
    }, [muted]);

    const handlePlay = async () => {
        const pc = new RTCPeerConnection();
        setPeerConnection(pc);
        pc.addTransceiver('video', { direction: 'recvonly' });
        pc.addTransceiver('audio', { direction: 'recvonly' });
        pc.ontrack = ev => {
            if (ev.track.kind === 'video' && ev.streams.length > 0) {
                if (refVideo.current) {
                    refVideo.current.srcObject = ev.streams[0];
                }
            }

            if (ev.track.kind === 'audio') {
                refAudio.current.srcObject = new MediaStream([ev.track]);
            }
        };
        pc.onconnectionstatechange = () => {
            switch (pc.connectionState) {
                case 'disconnected': {
                    handleStop();
                    break;
                }
            }
        };
        const whep = new WHEPClient();
        setWhepClient(whep);
        const url = `${location.origin}/whep/${streamId}`;
        const token = '';
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
        if (whepClient) {
            await whepClient.stop();
            setWhepClient(null);
        }
        if (peerConnection) {
            setPeerConnection(null);
        }
        if (reconnect > 0) {
            setTimeout(() => { handleReconnect(); }, reconnect);
        }
    };

    const handleReconnect = async () => {
        await handlePlay();
        refVideo.current?.play();
    };

    const handleVideoClick = () => {
        if (!whepClient) {
            handlePlay();
        }
    };

    return (
        <div id="player">
            <video ref={refVideo} controls={controls} onClick={() => handleVideoClick()}></video>
        </div>
    );
}
