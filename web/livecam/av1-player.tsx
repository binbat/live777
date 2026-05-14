import { useEffect, useRef } from 'preact/hooks';
import dashjs from 'dashjs';

export function Av1Player({ src }: { src: string }) {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    if (!videoRef.current) return;
    const player = dashjs.MediaPlayer().create();
    player.initialize(videoRef.current, src, true);
    // 让 Dash.js 自动选择浏览器原生 AV1，若不支持会回退到 H.264
    player.updateSettings({ streaming: { buffer: { bufferPruningInterval: 30 } } });
    return () => player.reset();
  }, [src]);

  return <video ref={videoRef} className="w-full h-full object-cover" controls autoPlay playsInline />;
}
