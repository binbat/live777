import { forwardRef, useImperativeHandle, useState, useEffect, useCallback } from 'preact/compat';
import { Alert, Button, Card, Badge, Modal, Table } from 'react-daisyui';
import { 
    CogIcon, 
    WifiIcon, 
    GlobeAltIcon, 
    ClockIcon,
    ServerIcon,
    ArrowPathIcon,
    InformationCircleIcon,
    PlayIcon,
    StopIcon,
    ExclamationTriangleIcon
} from '@heroicons/react/24/outline';
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

interface SystemInfo {
    platform_type: string;
    arch: string;
    os: string;
    kernel: string;
    hostname: string;
}

interface NetworkInterface {
    name: string;
    ip?: string;
    netmask?: string;
    gateway?: string;
    dns: string[];
    dhcp_enabled: boolean;
    status: 'Up' | 'Down' | 'Unknown';
}

interface NetworkInfoResponse {
    system: SystemInfo;
    interfaces: NetworkInterface[];
}

interface ApiResponse<T> {
    success: boolean;
    message: string;
    data?: T;
}

const RESOLUTION_OPTIONS = [
    { value: '1920x1080', label: '1080p (1920x1080)' },
    { value: '1280x720', label: '720p (1280x720)' },
    { value: '640x480', label: '480p (640x480)' },
    { value: '320x240', label: '240p (320x240)' },
] as const;

const TIMEZONE_OPTIONS = [
    { value: 'UTC', label: 'UTC' },
    { value: 'Asia/Shanghai', label: 'Asia/Shanghai (北京时间)' },
    { value: 'America/New_York', label: 'America/New_York (东部时间)' },
    { value: 'Europe/London', label: 'Europe/London (伦敦时间)' },
    { value: 'Asia/Tokyo', label: 'Asia/Tokyo (东京时间)' },
] as const;

const DEFAULT_CONFIG: NetworkConfigData = {
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
};

const DEFAULT_DHCP_CONFIG = {
    interface: '',
    range_start: '192.168.42.100',
    range_end: '192.168.42.200'
};

export const NetworkConfig = forwardRef<NetworkConfigRef, NetworkConfigProps>(({ className }, ref) => {
    const { token } = useAuth();
    
    const [config, setConfig] = useState<NetworkConfigData>(DEFAULT_CONFIG);
    const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
    const [interfaces, setInterfaces] = useState<NetworkInterface[]>([]);
    const [activeTab, setActiveTab] = useState<'basic' | 'interfaces' | 'dhcp'>('basic');
    
    const [isLoading, setIsLoading] = useState(false);
    const [isSaving, setIsSaving] = useState(false);
    
    const [error, setError] = useState('');
    const [success, setSuccess] = useState('');
    
    const [showInterfaceModal, setShowInterfaceModal] = useState(false);
    const [selectedInterface, setSelectedInterface] = useState<NetworkInterface | null>(null);
    
    const [dhcpConfig, setDhcpConfig] = useState(DEFAULT_DHCP_CONFIG);

    const makeRequest = useCallback(async (url: string, options: RequestInit = {}) => {
        const defaultOptions: RequestInit = {
            headers: {
                'Content-Type': 'application/json',
                Authorization: `Bearer ${token}`,
            },
        };
        return fetch(url, { ...defaultOptions, ...options });
    }, [token]);

    const showMessage = useCallback((message: string, type: 'success' | 'error') => {
        if (type === 'success') {
            setSuccess(message);
            setError('');
            setTimeout(() => setSuccess(''), 3000);
        } else {
            setError(message);
            setSuccess('');
        }
    }, []);

    const handleApiResponse = useCallback(async (response: Response, successMessage: string) => {
        if (response.ok) {
            const result: ApiResponse<unknown> = await response.json();
            if (result.success) {
                showMessage(successMessage, 'success');
                return true;
            } else {
                showMessage(result.message || 'Operation failed', 'error');
                return false;
            }
        } else {
            showMessage('Network request failed', 'error');
            return false;
        }
    }, [showMessage]);

    const loadConfig = useCallback(async () => {
        setIsLoading(true);
        setError('');
        try {
            const response = await makeRequest('/api/network/config');
            if (response.ok) {
                const data = await response.json();
                setConfig(data);
            } else {
                showMessage('Failed to load network configuration', 'error');
            }
        } catch (err) {
            console.error('Load config error:', err);
            showMessage('Network error occurred while loading configuration', 'error');
        } finally {
            setIsLoading(false);
        }
    }, [makeRequest, showMessage]);

    const loadNetworkInfo = useCallback(async () => {
        try {
            const response = await makeRequest('/api/network/info');
            if (response.ok) {
                const result: ApiResponse<NetworkInfoResponse> = await response.json();
                if (result.success && result.data) {
                    setSystemInfo(result.data.system);
                    setInterfaces(result.data.interfaces);
                } else {
                    showMessage(result.message || 'Failed to load network info', 'error');
                }
            } else {
                showMessage('Failed to load network information', 'error');
            }
        } catch (err) {
            console.error('Load network info error:', err);
            showMessage('Network error occurred while loading network information', 'error');
        }
    }, [makeRequest, showMessage]);

    const saveConfig = useCallback(async () => {
        setIsSaving(true);
        try {
            const response = await makeRequest('/api/network/config', {
                method: 'POST',
                body: JSON.stringify(config),
            });
            await handleApiResponse(response, 'Network configuration saved successfully!');
        } catch (err) {
            console.error('Save config error:', err);
            showMessage('Network error occurred while saving configuration', 'error');
        } finally {
            setIsSaving(false);
        }
    }, [config, makeRequest, handleApiResponse, showMessage]);

    const refreshNetworkInfo = useCallback(async () => {
        await Promise.all([loadConfig(), loadNetworkInfo()]);
    }, [loadConfig, loadNetworkInfo]);

    const configureInterface = useCallback(async (interfaceConfig: NetworkInterface) => {
        try {
            const response = await makeRequest(`/api/network/interfaces/${interfaceConfig.name}`, {
                method: 'PUT',
                body: JSON.stringify(interfaceConfig),
            });
            
            if (await handleApiResponse(response, 'Interface configured successfully!')) {
                await loadNetworkInfo();
                setShowInterfaceModal(false);
            }
        } catch (err) {
            console.error('Configure interface error:', err);
            showMessage('Network error occurred while configuring interface', 'error');
        }
    }, [makeRequest, handleApiResponse, loadNetworkInfo, showMessage]);

    const toggleInterface = useCallback(async (interfaceName: string, up: boolean) => {
        try {
            const response = await makeRequest(`/api/network/interfaces/${interfaceName}/state`, {
                method: 'POST',
                body: JSON.stringify({ up }),
            });
            
            if (await handleApiResponse(response, `Interface ${up ? 'enabled' : 'disabled'} successfully!`)) {
                await loadNetworkInfo();
            }
        } catch (err) {
            console.error('Toggle interface error:', err);
            showMessage('Network error occurred while toggling interface', 'error');
        }
    }, [makeRequest, handleApiResponse, loadNetworkInfo, showMessage]);

    const configureDHCP = useCallback(async () => {
        try {
            const response = await makeRequest('/api/network/dhcp', {
                method: 'POST',
                body: JSON.stringify(dhcpConfig),
            });
            await handleApiResponse(response, 'DHCP server configured successfully!');
        } catch (err) {
            console.error('Configure DHCP error:', err);
            showMessage('Network error occurred while configuring DHCP', 'error');
        }
    }, [dhcpConfig, makeRequest, handleApiResponse, showMessage]);

    const controlDHCP = useCallback(async (start: boolean) => {
        try {
            const response = await makeRequest('/api/network/dhcp/control', {
                method: 'POST',
                body: JSON.stringify({ start }),
            });
            await handleApiResponse(response, `DHCP service ${start ? 'started' : 'stopped'} successfully!`);
        } catch (err) {
            console.error('Control DHCP error:', err);
            showMessage(`Network error occurred while ${start ? 'starting' : 'stopping'} DHCP service`, 'error');
        }
    }, [makeRequest, handleApiResponse, showMessage]);

    const syncTime = useCallback(async () => {
        try {
            const response = await makeRequest('/api/network/ntp/sync', {
                method: 'POST',
                body: JSON.stringify({ server: config.ntp.server }),
            });
            await handleApiResponse(response, 'Time synchronized successfully!');
        } catch (err) {
            console.error('Sync time error:', err);
            showMessage('Network error occurred while syncing time', 'error');
        }
    }, [config.ntp.server, makeRequest, handleApiResponse, showMessage]);

    const restartNetwork = useCallback(async () => {
        if (!confirm('Are you sure you want to restart the network service? This may interrupt network connectivity.')) {
            return;
        }

        try {
            const response = await makeRequest('/api/network/restart', {
                method: 'POST',
            });
            
            if (await handleApiResponse(response, 'Network service restarted successfully!')) {
                setTimeout(() => refreshNetworkInfo(), 3000);
            }
        } catch (err) {
            console.error('Restart network error:', err);
            showMessage('Network error occurred while restarting network service', 'error');
        }
    }, [makeRequest, handleApiResponse, refreshNetworkInfo, showMessage]);

    const updateConfig = useCallback((path: string, value: string | number | boolean) => {
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
    }, []);

    const getStatusBadge = useCallback((status: NetworkInterface['status']) => {
        const statusConfig = {
            Up: { color: 'success' as const, text: 'UP' },
            Down: { color: 'error' as const, text: 'DOWN' },
            Unknown: { color: 'warning' as const, text: 'UNKNOWN' },
        };
        
        const config = statusConfig[status];
        return <Badge color={config.color}>{config.text}</Badge>;
    }, []);

    const openInterfaceModal = useCallback((iface: NetworkInterface) => {
        setSelectedInterface({ ...iface });
        setShowInterfaceModal(true);
    }, []);

    useImperativeHandle(ref, () => ({
        refresh: refreshNetworkInfo,
    }), [refreshNetworkInfo]);

    useEffect(() => {
        if (token) {
            refreshNetworkInfo();
        }
    }, [token, refreshNetworkInfo]);

    if (isLoading) {
        return (
            <div className="flex justify-center items-center p-8">
                <span className="loading loading-lg loading-spinner text-primary"></span>
            </div>
        );
    }

    return (
        <div className={`space-y-6 ${className || ''}`}>
            <div className="flex items-center justify-between mb-4">
                <div className="flex items-center gap-2">
                    <CogIcon className="w-6 h-6" />
                    <h2 className="text-2xl font-bold">Network Configuration</h2>
                </div>
                <div className="flex gap-2">
                    <Button 
                        size="sm" 
                        onClick={refreshNetworkInfo}
                        className="gap-2"
                    >
                        <ArrowPathIcon className="w-4 h-4" />
                        Refresh
                    </Button>
                    <Button 
                        size="sm" 
                        color="warning" 
                        onClick={restartNetwork}
                        className="gap-2"
                    >
                        <ExclamationTriangleIcon className="w-4 h-4" />
                        Restart Network
                    </Button>
                </div>
            </div>

            {systemInfo && (
                <Card className="bg-base-200">
                    <Card.Body>
                        <Card.Title className="flex items-center gap-2">
                            <InformationCircleIcon className="w-5 h-5" />
                            System Information
                        </Card.Title>
                        <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                            <div>
                                <span className="font-semibold">Platform:</span>
                                <div className="text-gray-600">{systemInfo.platform_type}</div>
                            </div>
                            <div>
                                <span className="font-semibold">Architecture:</span>
                                <div className="text-gray-600">{systemInfo.arch}</div>
                            </div>
                            <div>
                                <span className="font-semibold">Kernel:</span>
                                <div className="text-gray-600">{systemInfo.kernel}</div>
                            </div>
                            <div>
                                <span className="font-semibold">Hostname:</span>
                                <div className="text-gray-600">{systemInfo.hostname}</div>
                            </div>
                        </div>
                    </Card.Body>
                </Card>
            )}

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

            <div className="tabs tabs-boxed">
                <button 
                    className={`tab ${activeTab === 'basic' ? 'tab-active' : ''}`}
                    onClick={() => setActiveTab('basic')}
                >
                    Basic Config
                </button>
                <button 
                    className={`tab ${activeTab === 'interfaces' ? 'tab-active' : ''}`}
                    onClick={() => setActiveTab('interfaces')}
                >
                    Network Interfaces
                </button>
                <button 
                    className={`tab ${activeTab === 'dhcp' ? 'tab-active' : ''}`}
                    onClick={() => setActiveTab('dhcp')}
                >
                    DHCP Server
                </button>
            </div>

            {activeTab === 'basic' && (
                <div className="space-y-6">
                    <Card className="bg-base-200">
                        <Card.Body>
                            <Card.Title className="flex items-center gap-2">
                                <ServerIcon className="w-5 h-5" />
                                Streaming Protocol
                            </Card.Title>
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
                            <Card.Title className="flex items-center gap-2">
                                <WifiIcon className="w-5 h-5" />
                                Static IP Configuration
                            </Card.Title>

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
                            <Card.Title className="flex items-center gap-2">
                                <ClockIcon className="w-5 h-5" />
                                NTP Configuration
                            </Card.Title>

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
                                <div className="space-y-4 mt-4">
                                    <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
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
                                    <div className="flex gap-2">
                                        <Button size="sm" onClick={syncTime} className="gap-2">
                                            <ClockIcon className="w-4 h-4" />
                                            Sync Time Now
                                        </Button>
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
                </div>
            )}

            {activeTab === 'interfaces' && (
                <Card className="bg-base-200">
                    <Card.Body>
                        <Card.Title className="flex items-center gap-2">
                            <GlobeAltIcon className="w-5 h-5" />
                            Network Interfaces
                        </Card.Title>
                        
                        {interfaces.length === 0 ? (
                            <div className="text-center py-8 text-gray-500">
                                No network interfaces found
                            </div>
                        ) : (
                            <div className="overflow-x-auto">
                                <Table className="table-zebra">
                                    <Table.Head>
                                        <span>Interface</span>
                                        <span>Status</span>
                                        <span>IP Address</span>
                                        <span>Netmask</span>
                                        <span>DHCP</span>
                                        <span>Actions</span>
                                    </Table.Head>
                                    <Table.Body>
                                        {interfaces.map((iface) => (
                                            <Table.Row key={iface.name}>
                                                <span className="font-mono font-bold">{iface.name}</span>
                                                <span>{getStatusBadge(iface.status)}</span>
                                                <span className="font-mono">{iface.ip || 'Not configured'}</span>
                                                <span className="font-mono">{iface.netmask || 'Not configured'}</span>
                                                <span>
                                                    <Badge color={iface.dhcp_enabled ? 'success' : 'ghost'}>
                                                        {iface.dhcp_enabled ? 'Enabled' : 'Disabled'}
                                                    </Badge>
                                                </span>
                                                <span className="flex gap-1">
                                                    <Button 
                                                        size="xs" 
                                                        onClick={() => openInterfaceModal(iface)}
                                                    >
                                                        Configure
                                                    </Button>
                                                    <Button 
                                                        size="xs" 
                                                        color={iface.status === 'Up' ? 'warning' : 'success'}
                                                        onClick={() => toggleInterface(iface.name, iface.status !== 'Up')}
                                                    >
                                                        {iface.status === 'Up' ? 'Disable' : 'Enable'}
                                                    </Button>
                                                </span>
                                            </Table.Row>
                                        ))}
                                    </Table.Body>
                                </Table>
                            </div>
                        )}
                    </Card.Body>
                </Card>
            )}

            {activeTab === 'dhcp' && (
                <Card className="bg-base-200">
                    <Card.Body>
                        <Card.Title className="flex items-center gap-2">
                            <ServerIcon className="w-5 h-5" />
                            DHCP Server Configuration
                        </Card.Title>
                        
                        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Interface</span>
                                </label>
                                <select
                                    className="select select-bordered"
                                    value={dhcpConfig.interface}
                                    onChange={(e) => setDhcpConfig(prev => ({ ...prev, interface: e.currentTarget.value }))}
                                >
                                    <option value="">Select Interface</option>
                                    {interfaces.map((iface) => (
                                        <option key={iface.name} value={iface.name}>
                                            {iface.name}
                                        </option>
                                    ))}
                                </select>
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Start IP</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="192.168.42.100"
                                    value={dhcpConfig.range_start}
                                    onInput={(e) => setDhcpConfig(prev => ({ ...prev, range_start: e.currentTarget.value }))}
                                />
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">End IP</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="192.168.42.200"
                                    value={dhcpConfig.range_end}
                                    onInput={(e) => setDhcpConfig(prev => ({ ...prev, range_end: e.currentTarget.value }))}
                                />
                            </div>
                        </div>
                        
                        <div className="flex gap-2 mt-4">
                            <Button onClick={configureDHCP} className="gap-2">
                                <CogIcon className="w-4 h-4" />
                                Configure DHCP
                            </Button>
                            <Button color="success" onClick={() => controlDHCP(true)} className="gap-2">
                                <PlayIcon className="w-4 h-4" />
                                Start Service
                            </Button>
                            <Button color="error" onClick={() => controlDHCP(false)} className="gap-2">
                                <StopIcon className="w-4 h-4" />
                                Stop Service
                            </Button>
                        </div>
                    </Card.Body>
                </Card>
            )}

            {activeTab === 'basic' && (
                <div className="flex justify-end gap-2">
                    <Button onClick={loadConfig} disabled={isSaving}>
                        Refresh
                    </Button>
                    <Button color="primary" onClick={saveConfig} disabled={isSaving}>
                        {isSaving && <span className="loading loading-spinner"></span>}
                        {isSaving ? 'Saving...' : 'Save Configuration'}
                    </Button>
                </div>
            )}

            <Modal open={showInterfaceModal}>
                <Modal.Header className="font-bold">
                    Configure Interface: {selectedInterface?.name}
                    <Button 
                        size="sm" 
                        shape="circle" 
                        className="absolute right-2 top-2"
                        onClick={() => setShowInterfaceModal(false)}
                    >
                        ✕
                    </Button>
                </Modal.Header>
                <Modal.Body>
                    {selectedInterface && (
                        <div className="space-y-4">
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">IP Address</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="192.168.1.100"
                                    value={selectedInterface.ip || ''}
                                    onInput={(e) => setSelectedInterface(prev => prev ? {
                                        ...prev,
                                        ip: e.currentTarget.value
                                    } : null)}
                                />
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Netmask</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="255.255.255.0"
                                    value={selectedInterface.netmask || ''}
                                    onInput={(e) => setSelectedInterface(prev => prev ? {
                                        ...prev,
                                        netmask: e.currentTarget.value
                                    } : null)}
                                />
                            </div>
                            <div className="form-control">
                                <label className="label">
                                    <span className="label-text">Gateway</span>
                                </label>
                                <input
                                    type="text"
                                    className="input input-bordered"
                                    placeholder="192.168.1.1"
                                    value={selectedInterface.gateway || ''}
                                    onInput={(e) => setSelectedInterface(prev => prev ? {
                                        ...prev,
                                        gateway: e.currentTarget.value
                                    } : null)}
                                />
                            </div>
                            <div className="form-control">
                                <label className="cursor-pointer label">
                                    <span className="label-text">Enable DHCP</span>
                                    <input
                                        type="checkbox"
                                        className="checkbox checkbox-primary"
                                        checked={selectedInterface.dhcp_enabled}
                                        onChange={(e) => setSelectedInterface(prev => prev ? {
                                            ...prev,
                                            dhcp_enabled: e.currentTarget.checked
                                        } : null)}
                                    />
                                </label>
                            </div>
                        </div>
                    )}
                </Modal.Body>
                <Modal.Actions>
                    <Button onClick={() => setShowInterfaceModal(false)}>
                        Cancel
                    </Button>
                    <Button 
                        color="primary" 
                        onClick={() => selectedInterface && configureInterface(selectedInterface)}
                    >
                        Apply Configuration
                    </Button>
                </Modal.Actions>
            </Modal>
        </div>
    );
});

