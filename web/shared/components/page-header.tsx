import { useContext } from 'preact/hooks';
import { Button, Dropdown, Navbar, Tabs } from 'react-daisyui';
import { ChevronDownIcon } from '@heroicons/react/24/solid';
import { Monitor, Calendar } from 'lucide-react';

import Logo from '/logo.svg';
import { TokenContext } from '../context';

interface PageHeaderProps {
    currentView?: string;
    onNavigate?: (view: string) => void;
    enabledTools?: {
        debugger?: boolean;
        player?: boolean;
        dash?: boolean;
    };
}

export function PageHeader({ currentView, onNavigate, enabledTools }: PageHeaderProps) {
    const tokenContext = useContext(TokenContext);
    const tools = {
        debugger: true,
        player: true,
        dash: true,
        ...enabledTools,
    };

    const handleOpenDebuggerPage = () => {
        const params = new URLSearchParams();
        params.set('token', tokenContext.token);
        const url = new URL(`/tools/debugger.html?${params.toString()}`, location.origin);
        window.open(url);
    };

    const handleOpenPlayerPage = () => {
        const params = new URLSearchParams();
        params.set('id', '');
        params.set('autoplay', '');
        params.set('muted', '');
        params.set('reconnect', '3000');
        params.set('token', tokenContext.token);
        const url = new URL(`/tools/player.html?${params.toString()}`, location.origin);
        window.open(url);
    };

    const handleOpenDashPage = () => {
        const url = new URL('/tools/dash.html', location.origin);
        window.open(url);
    };

    const toolItems: Array<{ key: string; label: string; onClick: () => void; hidden?: boolean }> = [
        { key: 'debugger', label: 'Debugger', onClick: handleOpenDebuggerPage, hidden: !tools.debugger },
        { key: 'player', label: 'Player', onClick: handleOpenPlayerPage, hidden: !tools.player },
        { key: 'dash', label: 'DASH Player', onClick: handleOpenDashPage, hidden: !tools.dash },
    ];
    const visibleItems = toolItems.filter(item => !item.hidden);

    return (
        <Navbar className="bg-base-300 px-0">
            <div className="flex grow max-w-screen-xl px-4 mx-auto">
                <div className="flex gap-2 mr-auto group">
                    <img
                        src={Logo}
                        className="h-8 transition-[filter] duration-200 ease-in-out group-hover:drop-shadow-[0_0_1em_#1991e8aa]"
                    />
                    <span class="text-xl font-bold">Live777</span>
                </div>

                {/* Navigation Tabs */}
                {onNavigate && (
                    <div className="flex-1 flex justify-center">
                        <Tabs variant="boxed" size="sm">
                            <Tabs.Tab
                                active={currentView === 'streams'}
                                onClick={() => onNavigate('streams')}
                            >
                                <Monitor className="w-4 h-4 mr-2" />
                                Streams
                            </Tabs.Tab>
                            <Tabs.Tab
                                active={currentView === 'recordings'}
                                onClick={() => onNavigate('recordings')}
                            >
                                <Calendar className="w-4 h-4 mr-2" />
                                Recordings
                            </Tabs.Tab>
                        </Tabs>
                    </div>
                )}

                {visibleItems.length > 0 ? (
                    <Dropdown end>
                        <Button
                            tag="label"
                            color="ghost"
                            tabIndex={1}
                            endIcon={<ChevronDownIcon className="size-4 stroke-current" />}
                        >Tools</Button>
                        <Dropdown.Menu className="bg-base-300 mt-4 z-10">
                            {visibleItems.map(item => (
                                <Dropdown.Item key={item.key} onClick={item.onClick}>{item.label}</Dropdown.Item>
                            ))}
                        </Dropdown.Menu>
                    </Dropdown>
                ) : null}
            </div>
        </Navbar>
    );
}
