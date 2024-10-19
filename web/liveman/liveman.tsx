import { useCallback, useRef, useState } from 'preact/hooks';

import type { Stream } from '../shared/api';
import { TokenContext } from '../shared/context';
import { Live777Logo } from '../shared/components/live777-logo';
import { StreamsTable } from '../shared/components/streams-table';
import { useNeedAuthorization } from '../shared/hooks/use-need-authorization';

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
            <button onClick={() => refStreamTokenDialog?.current?.show(stream.id)}>Create token</button>
        );
    }, []);

    return (
        <TokenContext.Provider value={{ token }}>
            <Live777Logo />
            <NodesTable />
            <StreamsTable renderExtraActions={renderCreateToken} />
            <StreamTokenDialog ref={refStreamTokenDialog} />
            <Login
                show={needsAuthorizaiton}
                onSuccess={onLoginSuccess}
            />
        </TokenContext.Provider>
    );
}
