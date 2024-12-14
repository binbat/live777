import { useCallback, useRef, useState } from 'preact/hooks';
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

    const refStreamTokenDialog = useRef<IStreamTokenDialog>(null);
    const renderCreateToken = useCallback((stream: Stream) => {
        return (
            <Button size="sm" onClick={() => refStreamTokenDialog?.current?.show(stream.id)}>Create token</Button>
        );
    }, []);

    return (
        <>
            <PageLayout token={token}>
                <NodesTable />
                <StreamsTable renderExtraActions={renderCreateToken} />
            </PageLayout>
            <StreamTokenDialog ref={refStreamTokenDialog} />
            <Login
                show={needsAuthorizaiton}
                onSuccess={onLoginSuccess}
            />
        </>
    );
}
