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
    const [currentView, setCurrentView] = useState<'streams' | 'recordings'>('streams');

    // Initialize view from URL params
    useEffect(() => {
        const params = new URLSearchParams(location.search);
        const view = params.get('view') as 'streams' | 'recordings';

        if (view) setCurrentView(view);


        const handlePopState = () => {
            const newParams = new URLSearchParams(location.search);
            const newView = newParams.get('view') as 'streams' | 'recordings' || 'streams';

            setCurrentView(newView);

        };

        window.addEventListener('popstate', handlePopState);
        return () => window.removeEventListener('popstate', handlePopState);
    }, []);

    const navigateToView = (view: 'streams' | 'recordings') => {
        const url = new URL(window.location.href);
        url.searchParams.set('view', view);
        window.history.pushState({}, '', url.toString());
        setCurrentView(view);
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
            default:
                return (
                    <>
                        {filterNodes.length > 0 ? null : <NodesTable />}
                        <StreamsTable
                            getStreams={getStreams}
                            getWhepUrl={streamId => getWhxpUrl('whep', streamId)}
                            getWhipUrl={streamId => getWhxpUrl('whip', streamId)}
                            features={{ autoDetectRecording: true }}
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
                onNavigate={(v: string) => navigateToView(v as 'streams' | 'recordings')}
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
