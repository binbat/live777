import { useState, useRef, useEffect } from 'preact/hooks';
import { useAuth } from './AuthContext';
import { WHEPClient } from '@binbat/whip-whep/whep.js';
import { PlayIcon, StopIcon, ExclamationTriangleIcon, CameraIcon } from '@heroicons/react/24/solid';

interface LiveCamViewerProps {
  streamId: string;
  autoPlay?: boolean;
  muted?: boolean;
  reconnectDelay?: number;
  getWhepUrl?: (streamId: string) => string;
  isVisible?: boolean; 
}

export function LiveCamViewer({
  streamId,
  autoPlay = true,
  muted = true,
  reconnectDelay = 3000, 
  getWhepUrl,
  isVisible = true, 
}: LiveCamViewerProps) {
  const { token } = useAuth();
  const videoRef = useRef<HTMLVideoElement>(null);

  const whepClientRef = useRef<WHEPClient | null>(null);
  const reconnectTimerRef = useRef<number | null>(null);
  const isCleanupRef = useRef(false);
  const wasVisibleRef = useRef(isVisible); 

  const [isConnected, setIsConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isConnecting, setIsConnecting] = useState(false);

  const handleStop = async (isGraceful = true, forceCleanup = false) => {
    console.log(`Stopping connection (graceful: ${isGraceful}, forceCleanup: ${forceCleanup})`);
    
    if (reconnectTimerRef.current) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    
    if (!forceCleanup && !isVisible && isConnected) {
      console.log('Component hidden but keeping connection alive...');
      return;
    }
    
    if (whepClientRef.current) {
      try {
        await whepClientRef.current.stop();
      } catch (e) {
        console.error('Error stopping WHEP client:', e);
      }
      whepClientRef.current = null;
    }
    
    if (videoRef.current) {
      videoRef.current.srcObject = null;
    }
    
    setIsConnected(false);
    setIsConnecting(false);
    
    if (!isGraceful && !isCleanupRef.current && reconnectDelay > 0 && isVisible) {
      console.log(`Connection lost. Reconnecting in ${reconnectDelay}ms...`);
      reconnectTimerRef.current = window.setTimeout(() => {
        if (!isCleanupRef.current && isVisible) {
          handlePlay();
        }
      }, reconnectDelay);
    }
  };

  const handlePlay = async () => {
    if (!streamId || !token) {
        setError('Missing Stream ID or authentication Token');
        return;
    }
    
    if (!isVisible) {
        console.log('Component not visible, skipping connection...');
        return;
    }
    
    if (isConnecting || isConnected || isCleanupRef.current) return;

    console.log(`Attempting to play stream: ${streamId}`);
    setIsConnecting(true);
    setError(null);

    try {
      const pc = new RTCPeerConnection({
        iceServers: [
          { urls: 'stun:stun.l.google.com:19302' }
        ]
      });
      
      pc.addTransceiver('video', { direction: 'recvonly' });

      pc.ontrack = (event) => {
        console.log(`Received track: ${event.track.kind}`);
        if (videoRef.current && !isCleanupRef.current && isVisible) {
          if (!videoRef.current.srcObject) {
            videoRef.current.srcObject = new MediaStream();
          }
          (videoRef.current.srcObject as MediaStream).addTrack(event.track);
        }
      };

      pc.oniceconnectionstatechange = () => {
        console.log(`ICE connection state: ${pc.iceConnectionState}`);
        if (isCleanupRef.current) return;
        
        switch (pc.iceConnectionState) {
            case 'connected':
            case 'completed':
                setIsConnected(true);
                setIsConnecting(false);
                setError(null);
                if (reconnectTimerRef.current) {
                    clearTimeout(reconnectTimerRef.current);
                    reconnectTimerRef.current = null;
                }
                break;
            case 'failed':
            case 'disconnected':
                if (isConnected) { 
                    setError('Connection lost');
                    handleStop(false);
                }
                break;
            case 'closed':
                if (isConnected) {
                    handleStop(true);
                }
                break;
        }
      };
      
      const whepClient = new WHEPClient();
      whepClientRef.current = whepClient;
      
      const baseUrl = import.meta.env.VITE_API_BASE_URL || window.location.origin;
      const url = getWhepUrl ? getWhepUrl(streamId) : `${baseUrl}/api/whep/${streamId}`;
      
      console.log(`Connecting to WHEP URL: ${url}`);
      await whepClient.view(pc, url, token);
      
    } catch (e: unknown) {
      console.error('WHEP connection failed:', e);
      if (!isCleanupRef.current) {
        const errorMessage = e instanceof Error ? e.message : 'Failed to start playback';
        setError(errorMessage);
        await handleStop(false);
      }
    }
  };

  const handleScreenshot = () => {
    if (!videoRef.current || !isConnected || videoRef.current.videoWidth === 0) {
      console.warn('Screenshot failed: Video not ready or not connected.');
      return;
    }
    const video = videoRef.current;
    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
        console.error('Failed to get canvas context.');
        return;
    }
    ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
    const filename = `screenshot-${streamId}-${timestamp}.jpg`;
    const link = document.createElement('a');
    link.href = canvas.toDataURL('image/jpeg', 0.9);
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  };

  useEffect(() => {
    const wasVisible = wasVisibleRef.current;
    wasVisibleRef.current = isVisible;

    if (!wasVisible && isVisible && autoPlay) {
      console.log('Component became visible, starting connection...');
      handlePlay();
    } else if (wasVisible && !isVisible) {
      console.log('Component became hidden, pausing connection...');
      if (videoRef.current) {
        videoRef.current.pause();
      }
    }
  }, [isVisible]);

  useEffect(() => {
    isCleanupRef.current = false;
    
    if (autoPlay && isVisible) {
      handlePlay();
    }
    
    return () => {
      console.log('LiveCamViewer unmounting, cleaning up...');
      isCleanupRef.current = true;
      handleStop(true, true); 
    };
  }, [streamId, token]); 

  return (
    <div className="bg-base-300 rounded-lg overflow-hidden shadow-lg">
      <div className="relative aspect-video">
        <video 
          ref={videoRef} 
          className="w-full h-full bg-black" 
          autoPlay={isVisible} 
          muted={muted} 
          playsInline 
          controls={false}
          onLoadedMetadata={() => {
            console.log('Video metadata loaded');
            if (isVisible && videoRef.current) {
              videoRef.current.play().catch(console.error);
            }
          }}
        />
        <div className={`absolute inset-0 flex items-center justify-center bg-black bg-opacity-60 transition-opacity duration-300 ${isConnected && isVisible ? 'opacity-0 pointer-events-none' : 'opacity-100'}`}>
          {!isVisible && (
            <div className="text-center text-white p-4">
              <div className="h-12 w-12 bg-gray-600 rounded-full mx-auto mb-2 flex items-center justify-center">
                <span className="text-xs">PAUSED</span>
              </div>
              <p className="font-bold">Stream Paused</p>
              <p className="text-sm">Switch to Live tab to resume</p>
            </div>
          )}
          {isVisible && !isConnected && !isConnecting && error && (
            <div className="text-center text-white p-4">
              <ExclamationTriangleIcon className="h-12 w-12 text-error mx-auto mb-2" />
              <p className="font-bold">Connection Error</p>
              <p className="text-sm">{error}</p>
              <button 
                className="btn btn-primary btn-sm mt-2" 
                onClick={handlePlay}
                disabled={isConnecting}
              >
                Retry
              </button>
            </div>
          )}
          {isVisible && isConnecting && (
            <div className="text-center text-white">
              <span className="loading loading-lg loading-spinner text-primary"></span>
              <p className="mt-2">Connecting...</p>
            </div>
          )}
        </div>
      </div>
      <div className="p-4 bg-base-200 flex items-center justify-between">
        <div>
          <p className="font-bold truncate" title={streamId}>Stream ID: {streamId || 'N/A'}</p>
          <p className={`text-sm font-semibold ${
            !isVisible ? 'text-warning' : 
            isConnected ? 'text-success' : 
            (error ? 'text-error' : (isConnecting ? 'text-warning' : 'text-base-content'))
          }`}>
            Status: {
              !isVisible ? 'Paused' :
              isConnected ? 'Connected' : 
              (error ? 'Error' : (isConnecting ? 'Connecting...' : 'Disconnected'))
            }
          </p>
        </div>
        <div className="flex items-center gap-2">
            <button 
              className="btn btn-ghost btn-sm btn-circle" 
              onClick={handleScreenshot} 
              disabled={!isConnected || !isVisible} 
              title="Screenshot"
            >
              <CameraIcon className={`h-5 w-5 ${(!isConnected || !isVisible) ? 'text-gray-600' : ''}`} />
            </button>
            {isConnected && isVisible ? (
              <button 
                className="btn btn-ghost btn-sm btn-circle" 
                onClick={() => handleStop(true, true)} 
                title="Stop"
              >
                <StopIcon className="h-6 w-6 text-error" />
              </button>
            ) : (
              <button 
                className="btn btn-ghost btn-sm btn-circle" 
                onClick={handlePlay} 
                disabled={isConnecting || !isVisible} 
                title="Play"
              >
                <PlayIcon className={`h-6 w-6 ${isVisible ? 'text-success' : 'text-gray-600'}`} />
              </button>
            )}
        </div>
      </div>
    </div>
  );
}
