import { useCallback, useEffect, useState } from 'preact/hooks';

import { type AuthorizationCallbacks } from '../authorization-middleware';

export function useNeedAuthorization(auth: Omit<AuthorizationCallbacks, 'setAuthorization'>) {
    const needsAuthorizaiton = useState(false);
    const cb = useCallback(() => {
        needsAuthorizaiton[1](true);
    }, []);

    useEffect(() => {
        auth.addUnauthorizedCallback(cb);
        return () => auth.removeUnauthorizedCallback(cb);
    }, []);

    return needsAuthorizaiton;
}
