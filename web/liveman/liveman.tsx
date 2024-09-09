import { useCallback, useEffect, useState } from 'preact/hooks';

import { addUnauthorizedCallback, removeUnauthorizedCallback } from './api';

import { Live777Logo } from '../shared/components/live777-logo';
import { Login } from './login';
import { NodesTable } from './nodes-table';
import { StreamsTable } from '../shared/components/streams-table';

export function Liveman() {
    const [needsAuthorizaiton, setNeedsAuthorizaiton] = useState(false);
    const unauthorizedCallback = useCallback(() => {
        setNeedsAuthorizaiton(true);
    }, []);

    useEffect(() => {
        addUnauthorizedCallback(unauthorizedCallback);
        return () => removeUnauthorizedCallback(unauthorizedCallback);
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
                    <StreamsTable cascade={false} />
                </>
            )}

        </>
    );
}
