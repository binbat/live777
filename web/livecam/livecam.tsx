import { useState, useRef} from 'preact/hooks';
import { Input } from 'react-daisyui';
import { LiveCamViewer } from './components/LiveViewer';
import { PlaybackViewer } from './components/Playback'; 
import { NetworkConfig, type NetworkConfigRef } from './components/NetworkConfig';
import { useAuth } from './components/AuthContext';
import { ChangePassword, type ChangePasswordRef } from './components/ChangePassword';

export function LiveCamPage(_props: { path: string }) {
    const [streamId, setStreamId] = useState('camera');
    const [mode, setMode] = useState<'live' | 'playback' | 'network'>('live'); 
    const { logout } = useAuth();

    const changePasswordModalRef = useRef<ChangePasswordRef>(null);
    const networkConfigRef = useRef<NetworkConfigRef>(null);

    return (
        <div className="min-h-screen bg-base-100 text-base-content">
            <div className="container mx-auto p-4 md:p-8">
                <div className="flex justify-between items-center mb-6">
                    <h1 className="text-3xl font-bold">LiveCam</h1>
                    <div className="flex items-center gap-2">
                        <button 
                            onClick={() => changePasswordModalRef.current?.show()} 
                            className="btn btn-outline btn-sm"
                        >
                            Change Password
                        </button>
                        <button onClick={logout} className="btn btn-outline btn-sm">Logout</button>
                    </div>
                </div>

                <div className="mb-6 flex flex-wrap gap-4 items-end">
                    <div className="form-control">
                        <label className="label">
                            <span className="label-text">Enter Stream ID</span>
                        </label>
                        <Input
                            className="w-full max-w-xs"
                            value={streamId}
                            onInput={(e) => setStreamId(e.currentTarget.value)}
                            placeholder="e.g., demo"
                        />
                    </div>
                    <div className="join">
                        <button 
                            className={`btn join-item ${mode === 'live' ? 'btn-active' : ''}`}
                            onClick={() => setMode('live')}>
                            Live
                        </button>
                        <button 
                            className={`btn join-item ${mode === 'playback' ? 'btn-active' : ''}`}
                            onClick={() => setMode('playback')}>
                            Playback
                        </button>
                        <button 
                            className={`btn join-item ${mode === 'network' ? 'btn-active' : ''}`}
                            onClick={() => setMode('network')}>
                            Network Config
                        </button>
                    </div>
                </div>

                {streamId ? (
                    <div>
                        {/* 保持 LiveCamViewer 始终挂载，通过 CSS 控制显示/隐藏 */}
                        <div 
                            className={`transition-opacity duration-300 ${
                                mode === 'live' ? 'opacity-100 block' : 'opacity-0 hidden'
                            }`}
                        >
                            <LiveCamViewer 
                                streamId={streamId} 
                                autoPlay={mode === 'live'}
                                isVisible={mode === 'live'}
                            />
                        </div>
                        
                        {/* PlaybackViewer 只在需要时渲染 */}
                        {mode === 'playback' && (
                            <div className="transition-opacity duration-300 opacity-100">
                                <PlaybackViewer streamId={streamId} />
                            </div>
                        )}
                        
                        {/* NetworkConfig 只在需要时渲染 */}
                        {mode === 'network' && (
                            <div className="transition-opacity duration-300 opacity-100">
                                <NetworkConfig ref={networkConfigRef} />
                            </div>
                        )}
                    </div>
                ) : (
                    <div className="text-center p-10 bg-base-200 rounded-lg">
                        <p>Enter a Stream ID to start.</p>
                    </div>
                )}
            </div>
            <ChangePassword ref={changePasswordModalRef} />
        </div>
    );
}
