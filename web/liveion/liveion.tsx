import { useState } from 'preact/hooks';

import * as api from '../shared/api';
import { Live777Logo } from '../shared/components/live777-logo';
import { StreamsTable } from '../shared/components/streams-table';
import { TokenContext } from '../shared/context';
import { useNeedAuthorization } from '../shared/hooks/use-need-authorization';

import { Login } from './components/login';

export function Liveion() {
    const [token, setToken] = useState('');
    const [needsAuthorizaiton, setNeedsAuthorization] = useNeedAuthorization(api);

    const onLoginSuccess = (t: string) => {
        setToken(t);
        setNeedsAuthorization(false);
    };

    return (
        <TokenContext.Provider value={{ token }}>
            <Live777Logo />
            {needsAuthorizaiton ? (
                <Login onSuccess={onLoginSuccess} />
            ) : (
                <StreamsTable showCascade />
            )}
        </TokenContext.Provider>
    );
}
