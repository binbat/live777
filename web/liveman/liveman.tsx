import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { Button } from 'react-daisyui';

import { type Stream } from '@/shared/api';
import { useNeedAuthorization } from '@/shared/hooks/use-need-authorization';
import { StreamsTable } from '@/shared/components/streams-table';
import { PageLayout } from '@/shared/components/page-layout';

import * as api from './api';
import { Login } from './components/login';
import { NodesTable } from './components/nodes-table';
import { type IStreamTokenDialog, StreamTokenDialog } from './components/dialog-token';

export function Liveman() {
    const [token, setToken] = useState('');
    const [needsAuthorizaiton, setNeedsAuthorization] = useNeedAuthorization(api);
    const onLoginSuccess = (t: string) => {
        setToken(t);
        setNeedsAuthorization(false);
    };

    const [filterNodes, setFilterNodes] = useState<string[] | undefined>();
    useEffect(() => {
        const params = new URLSearchParams(location.search);
        setFilterNodes(params.getAll('nodes'));
    }, [location.search]);
    const getStreams = useCallback(async () => {
        const streams = await api.getStreams(filterNodes);
        return streams.sort((a, b) => a.createdAt - b.createdAt);
    }, [filterNodes]);
    const getWhxpUrl = (whxp: 'whep' | 'whip', streamId: string) => {
        let url = `/${whxp}/${streamId}`;
        if (filterNodes && filterNodes.length > 0) {
            const params = new URLSearchParams();
            filterNodes?.forEach(v => params.append('nodes', v));
            url += `?${params.toString()}`;
        }
        return new URL(url, location.origin).toString();
    };

    const refStreamTokenDialog = useRef<IStreamTokenDialog>(null);
    const renderCreateToken = useCallback((stream: Stream) => {
        return (
            <Button size="sm" onClick={() => refStreamTokenDialog?.current?.show(stream.id)}>Create token</Button>
        );
    }, []);

    return (
        <>
            <PageLayout token={token}>
                {filterNodes && filterNodes.length > 0 ? null : <NodesTable />}
                <StreamsTable
                    getStreams={getStreams}
                    getWhepUrl={streamId => getWhxpUrl('whep', streamId)}
                    getWhipUrl={streamId => getWhxpUrl('whip', streamId)}
                    renderExtraActions={renderCreateToken}
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
