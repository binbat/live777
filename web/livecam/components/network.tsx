import { forwardRef, useImperativeHandle, useState, useEffect } from 'preact/compat';
import { Alert, Button, Card } from 'react-daisyui';
import { CogIcon } from '@heroicons/react/24/outline';
import { useAuth } from './auth';

export interface NetworkConfigRef {
    refresh: () => void;
}

interface NetworkConfigProps {
    className?: string;
}

interface NetworkConfigData {
    protocol: 'rtp' | 'rtsp';
    static_ip: {
        enabled: boolean;
        ip: string;
        netmask: string;
        gateway: string;
        dns: string;
    };
    ntp: {
        enabled: boolean;
        server: string;
        timezone: string;
    };
    camera: {
        resolution: string;
        fps: number;
        bitrate: number;
    };
}

const RESOLUTION_OPTIONS = [
    { value: '1920x1080', label: '1080p (1920x1080)' },
    { value: '1280x720', label: '720p (1280x720)' },
    { value: '640x480', label: '480p (640x480)' },
    { value: '320x240', label: '240p (320x240)' },
];

const TIMEZONE_OPTIONS = [
    { value: 'UTC', label: 'UTC' },
    { value: 'Asia/Shanghai', label: 'Asia/Shanghai' },
    { value: 'America/New_York', label: 'America/New_York' },
    { value: 'Europe/London', label: 'Europe/London' },
    { value: 'Asia/Tokyo', label: 'Asia/Tokyo' },
];

export const NetworkConfig = forwardRef<NetworkConfigRef, NetworkConfigProps>((_props, ref) => {
    const { token } = useAuth();
    const [config, setConfig] = useState<NetworkConfigData>({
        protocol: 'rtp',
        static_ip: {
            enabled: false,
            ip: '',
            netmask: '255.255.255.0',
            gateway: '',
            dns: '8.8.8.8',
        },
        ntp: {
            enabled: true,
            server: 'pool.ntp.org',
            timezone: 'UTC',
        },
        camera: {
            resolution: '1280x720',
            fps: 30,
            bitrate: 2000,
        },
    });

    const [isLoading, setIsLoading] = useState(false);
    const [isSaving, setIsSaving] = useState(false);
    const [error, setError] = useState('');
    const [success, setSuccess] = useState('');

    const loadConfig = async () => {
        setIsLoading(true);
        setError('');
        try {
            const response = await fetch('/api/network/config', {
                headers: {
                    Authorization: `Bearer ${token}`,
                },
            });

            if (response.ok) {
                const data = await response.json();
                setConfig(data);
            } else {
                setError('Failed to load network configuration');
            }
        } catch (err) {
            console.error('Load config error:', err);
            setError('Network error occurred while loading configuration');
        } finally {
            setIsLoading(false);
        }
    };

    const saveConfig = async () => {
        setIsSaving(true);
        setError('');
        setSuccess('');

        try {
            const response = await fetch('/api/network/config', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                    Authorization: `Bearer ${token}`,
                },
                body: JSON.stringify(config),
            });

            if (response.ok) {
                setSuccess('Network configuration saved successfully!');
                setTimeout(() => setSuccess(''), 3000);
            } else {
                const errorData = await response.text();
                setError(errorData || 'Failed to save network configuration');
            }
        } catch (err) {
            console.error('Save config error:', err);
            setError('Network error occurred while saving configuration');
        } finally {
            setIsSaving(false);
        }
    };

    useImperativeHandle(ref, () => ({
        refresh: loadConfig,
    }));

    useEffect(() => {
        loadConfig();
    }, [token]);

    // 修复：使用具体的类型而不是 any
    const updateConfig = (path: string, value: string | number | boolean) => {
        setConfig((prev) => {
            const newConfig = { ...prev };
            const keys = path.split('.');
            let current: Record<string, unknown> = newConfig;

            for (let i = 0; i < keys.length - 1; i++) {
                current = current[keys[i]] as Record<string, unknown>;
            }
            current[keys[keys.length - 1]] = value;

            return newConfig;
        });
    };

    if (isLoading) {
        return (
            <div className="flex justify-center items-center p-8">
                <span className="loading loading-lg loading-spinner text-primary"></span>
            </div>
        );
    }

    return (
        <div className="space-y-6">
            <div className="flex items-center gap-2 mb-4">
                <CogIcon className="w-6 h-6" />
                <h2 className="text-2xl font-bold">Network Configuration</h2>
            </div>

            {error && (
                <Alert status="error">
                    <span>{error}</span>
                </Alert>
            )}

            {success && (
                <Alert status="success">
                    <span>{success}</span>
                </Alert>
            )}

            <Card className="bg-base-200">
                <Card.Body>
                    <Card.Title>Streaming Protocol</Card.Title>
                    <div className="form-control">
                        <label className="label">
                            <span className="label-text">Protocol</span>
                        </label>
                        <select
                            className="select select-bordered w-full max-w-xs"
                            value={config.protocol}
                            onChange={(e) => updateConfig('protocol', e.currentTarget.value)}
                        >
                            <option value="rtp">RTP</option>
                            <option value="rtsp">RTSP</option>
                        </select>
                    </div>
                </Card.Body>
            </Card>

            <Card className="bg-base-200">
                <Card.Body>
                    <Card.Title>Static IP Configuration</Card.Title>

                    <div className="form-control">
                        <label className="cursor-pointer label">
                            <span className="label-text">Enable Static IP</span>
                            <input
                                type="checkbox"
                                className="checkbox checkbox-primary"
                                checked={config.static_ip.enabled}
                                onChange={(e) => updateConfig('static_ip.enabled', e.currentTarget.checked)}
                            />
                        </label>
                    </div>

                    {config.static_ip.enabled && (
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mt-4">
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">USB Network IP Address</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="192.168.42.1"
                                    value={config.static_ip.ip}
                                    onInput={(e) => updateConfig('static_ip.ip', e.currentTarget.value)}
                                />
                                <div className="label">
                                    <span className="label-text-alt">Must be in 192.168.42.x range (2-242)</span>
                                </div>
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Netmask</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="255.255.255.0"
                                    value={config.static_ip.netmask}
                                    onInput={(e) => updateConfig('static_ip.netmask', e.currentTarget.value)}
                                    disabled
                                />
                                <div className="label">
                                    <span className="label-text-alt">Fixed for USB CDC-NCM</span>
                                </div>
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Gateway</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="192.168.42.1"
                                    value={config.static_ip.gateway}
                                    onInput={(e) => updateConfig('static_ip.gateway', e.currentTarget.value)}
                                />
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">DNS Server</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="8.8.8.8"
                                    value={config.static_ip.dns}
                                    onInput={(e) => updateConfig('static_ip.dns', e.currentTarget.value)}
                                />
                            </div>
                        </div>
                    )}
                </Card.Body>
            </Card>

            <Card className="bg-base-200">
                <Card.Body>
                    <Card.Title>NTP Configuration</Card.Title>

                    <div className="form-control">
                        <label className="cursor-pointer label">
                            <span className="label-text">Enable NTP</span>
                            <input
                                type="checkbox"
                                className="checkbox checkbox-primary"
                                checked={config.ntp.enabled}
                                onChange={(e) => updateConfig('ntp.enabled', e.currentTarget.checked)}
                            />
                        </label>
                    </div>

                    {config.ntp.enabled && (
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mt-4">
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">NTP Server</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="pool.ntp.org"
                                    value={config.ntp.server}
                                    onInput={(e) => updateConfig('ntp.server', e.currentTarget.value)}
                                />
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Timezone</span>
                                </label>
                                <select
                                    className="select select-bordered"
                                    value={config.ntp.timezone}
                                    onChange={(e) => updateConfig('ntp.timezone', e.currentTarget.value)}
                                >
                                    {TIMEZONE_OPTIONS.map((option) => (
                                        <option key={option.value} value={option.value}>
                                            {option.label}
                                        </option>
                                    ))}
                                </select>
                            </div>
                        </div>
                    )}
                </Card.Body>
            </Card>

            <Card className="bg-base-200">
                <Card.Body>
                    <Card.Title>Camera Configuration</Card.Title>

                    <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                        <div className="form-control">
                            <label className="label">
                                <span className="label-text">Resolution</span>
                            </label>
                            <select
                                className="select select-bordered"
                                value={config.camera.resolution}
                                onChange={(e) => updateConfig('camera.resolution', e.currentTarget.value)}
                            >
                                {RESOLUTION_OPTIONS.map((option) => (
                                    <option key={option.value} value={option.value}>
                                        {option.label}
                                    </option>
                                ))}
                            </select>
                        </div>
                        <div className="form-control">
                            <label className="label">
                                <span className="label-text">FPS</span>
                            </label>
                            <input
                                type="number"
                                className="input input-bordered"
                                min="1"
                                max="60"
                                value={config.camera.fps}
                                onInput={(e) => updateConfig('camera.fps', parseInt(e.currentTarget.value) || 30)}
                            />
                        </div>
                        <div className="form-control">
                            <label className="label">
                                <span className="label-text">Bitrate (kbps)</span>
                            </label>
                            <input
                                type="number"
                                className="input input-bordered"
                                min="100"
                                max="10000"
                                value={config.camera.bitrate}
                                onInput={(e) => updateConfig('camera.bitrate', parseInt(e.currentTarget.value) || 2000)}
                            />
                        </div>
                    </div>
                </Card.Body>
            </Card>

            <div className="flex justify-end gap-2">
                <Button onClick={loadConfig} disabled={isSaving}>
                    Refresh
                </Button>
                <Button color="primary" onClick={saveConfig} disabled={isSaving}>
                    {isSaving && <span className="loading loading-spinner"></span>}
                    {isSaving ? 'Saving...' : 'Save Configuration'}
                </Button>
            </div>
        </div>
    );
});
