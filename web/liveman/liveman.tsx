import { useCallback, useEffect, useRef, useState } from 'preact/hooks';

import { addUnauthorizedCallback, removeUnauthorizedCallback } from './api';

import type { Stream } from '../shared/api';
import { Live777Logo } from '../shared/components/live777-logo';
import { StreamsTable } from '../shared/components/streams-table';

import { Login } from './components/login';
import { NodesTable } from './components/nodes-table';
import { type INewStreamDialog, StreamTokenDialog } from './components/dialog-token';

export function Liveman() {
    const [needsAuthorizaiton, setNeedsAuthorizaiton] = useState(false);
    const unauthorizedCallback = useCallback(() => {
        setNeedsAuthorizaiton(true);
    }, []);

    useEffect(() => {
        addUnauthorizedCallback(unauthorizedCallback);
        return () => removeUnauthorizedCallback(unauthorizedCallback);
    }, []);

    const refStreamTokenDialog = useRef<INewStreamDialog>(null);
    const renderCreateToken = useCallback((stream: Stream) => {
        return (
            <button onClick={() => refStreamTokenDialog?.current?.show(stream.id)}>Create token</button>
        );
    }, []);

    return (
        <>
            <Live777Logo />
            {needsAuthorizaiton ? (
                <>
                    <Login onSuccess={() => setNeedsAuthorizaiton(false)} />
                </>
            ) : (
                <>
                    <NodesTable />
                    <StreamsTable cascade={false} renderExtraActions={renderCreateToken} />
                </>
            )}
            <StreamTokenDialog ref={refStreamTokenDialog} />
        </>
    );
}
