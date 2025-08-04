import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { Button } from 'react-daisyui';

import { useNeedAuthorization } from '@/shared/hooks/use-need-authorization';
import { StreamsTable } from '@/shared/components/streams-table';
import { PageLayout } from '@/shared/components/page-layout';
import * as sharedApi from '@/shared/api';

import * as livemanApi from './api';
import { type LoginProps, Login } from './components/login';
import { NodesTable } from './components/nodes-table';
import { type IStreamTokenDialog, StreamTokenDialog } from './components/dialog-token';
import { RecordingsPage } from './components/recordings-page';
import { PlaybackPage } from './components/playback-page';

const TOKEN_KEY = 'liveman_auth_token';
const savedToken = localStorage.getItem(TOKEN_KEY) ?? '';
const savedTokenValue = savedToken.split(' ')[1] ?? '';
if (savedToken) {
    livemanApi.setAuthToken(savedToken);
    sharedApi.setAuthToken(savedToken);
}

const initialNodes = new URLSearchParams(location.search).getAll('nodes');

export function Liveman() {
    const [token, setToken] = useState(savedTokenValue);
    const [needsAuthorizaiton, setNeedsAuthorization] = useNeedAuthorization(livemanApi);
    const onLoginSuccess: LoginProps['onSuccess'] = (tokenType, tokenValue) => {
        setToken(tokenValue);
        setNeedsAuthorization(false);
        localStorage.setItem(TOKEN_KEY, `${tokenType} ${tokenValue}`);
    };

    // View state management
    const [currentView, setCurrentView] = useState<'streams' | 'recordings' | 'playback'>('streams');
    const [playbackStream, setPlaybackStream] = useState<string>('');
    const [playbackSessionId, setPlaybackSessionId] = useState<string>('');

    // Initialize view from URL params
    useEffect(() => {
        const params = new URLSearchParams(location.search);
        const view = params.get('view') as 'streams' | 'recordings' | 'playback';
        const stream = params.get('stream');
        const sessionId = params.get('sessionId');
        
        if (view) setCurrentView(view);
        if (stream) setPlaybackStream(stream);
        if (sessionId) setPlaybackSessionId(sessionId);

        const handlePopState = () => {
            const newParams = new URLSearchParams(location.search);
            const newView = newParams.get('view') as 'streams' | 'recordings' | 'playback' || 'streams';
            const newStream = newParams.get('stream') || '';
            const newSessionId = newParams.get('sessionId') || '';
            
            setCurrentView(newView);
            setPlaybackStream(newStream);
            setPlaybackSessionId(newSessionId);
        };

        window.addEventListener('popstate', handlePopState);
        return () => window.removeEventListener('popstate', handlePopState);
    }, []);

    const navigateToView = (view: 'streams' | 'recordings' | 'playback', stream?: string) => {
        const url = new URL(window.location.href);
        url.searchParams.set('view', view);
        if (stream) {
            url.searchParams.set('stream', stream);
        } else {
            url.searchParams.delete('stream');
        }
        window.history.pushState({}, '', url.toString());
        setCurrentView(view);
        if (stream) setPlaybackStream(stream);
    };

    const [filterNodes, setFilterNodes] = useState<string[]>(initialNodes);
    useEffect(() => {
        const params = new URLSearchParams(location.search);
        setFilterNodes(params.getAll('nodes'));
    }, [location.search]);
    const getStreams = useCallback(async () => {
        try {
            const streams = await livemanApi.getStreams(filterNodes);
            return streams.sort((a, b) => a.createdAt - b.createdAt);
        } catch {
            return [];
        }
    }, filterNodes);
    const getWhxpUrl = (whxp: 'whep' | 'whip', streamId: string) => {
        let url = `/${whxp}/${streamId}`;
        if (filterNodes.length > 0) {
            const params = new URLSearchParams();
            filterNodes?.forEach(v => params.append('nodes', v));
            url += `?${params.toString()}`;
        }
        return new URL(url, location.origin).toString();
    };

    const refStreamTokenDialog = useRef<IStreamTokenDialog>(null);

    const renderCurrentView = () => {
        switch (currentView) {
            case 'recordings':
                return <RecordingsPage />;
            case 'playback':
                return (
                    <PlaybackPage 
                        streamId={playbackStream} 
                        sessionId={playbackSessionId}
                        onBack={() => navigateToView('recordings')} 
                    />
                );
            default:
                return (
                    <>
                        {filterNodes.length > 0 ? null : <NodesTable />}
                        <StreamsTable
                            getStreams={getStreams}
                            getWhepUrl={streamId => getWhxpUrl('whep', streamId)}
                            getWhipUrl={streamId => getWhxpUrl('whip', streamId)}
                            renderExtraActions={stream => (
                                <Button size="sm" onClick={() => refStreamTokenDialog?.current?.show(stream.id)}>Create token</Button>
                            )}
                        />
                    </>
                );
        }
    };

    return (
        <>
            <PageLayout 
                token={token}
                currentView={currentView}
                onNavigate={navigateToView}
            >
                {renderCurrentView()}
            </PageLayout>
            <StreamTokenDialog ref={refStreamTokenDialog} />
            <Login
                show={needsAuthorizaiton}
                onSuccess={onLoginSuccess}
            />
        </>
    );
}
