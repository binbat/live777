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

    const [filterNodes, setFilterNodes] = useState<string[]>(initialNodes);
    useEffect(() => {
        const params = new URLSearchParams(location.search);
        setFilterNodes(params.getAll('nodes'));
    }, [location.search]);
    const getStreams = useCallback(async () => {
        const streams = await livemanApi.getStreams(filterNodes);
        return streams.sort((a, b) => a.createdAt - b.createdAt);
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

    return (
        <>
            <PageLayout token={token}>
                {filterNodes.length > 0 ? null : <NodesTable />}
                <StreamsTable
                    getStreams={getStreams}
                    getWhepUrl={streamId => getWhxpUrl('whep', streamId)}
                    getWhipUrl={streamId => getWhxpUrl('whip', streamId)}
                    renderExtraActions={stream => (
                        <Button size="sm" onClick={() => refStreamTokenDialog?.current?.show(stream.id)}>Create token</Button>
                    )}
                />
            </PageLayout>
            <StreamTokenDialog ref={refStreamTokenDialog} />
            <Login
                show={needsAuthorizaiton}
                onSuccess={onLoginSuccess}
            />
        </>
    );
}
