import { useCallback, useEffect, useState } from 'preact/hooks';

import { type UnauthorizedCallback   } from '../authorization-middleware';

interface AuthorizationCallbacks {
    addUnauthorizedCallback: (cb: UnauthorizedCallback) => void;
    removeUnauthorizedCallback: (cb: UnauthorizedCallback) => boolean;
}

export function useNeedAuthorization(auth: AuthorizationCallbacks) {
    const needsAuthorizaiton = useState(false);
    const unauthorizedCallback = useCallback(() => {
        needsAuthorizaiton[1](true);
    }, []);

    useEffect(() => {
        auth.addUnauthorizedCallback(unauthorizedCallback);
        return () => auth.removeUnauthorizedCallback(unauthorizedCallback);
    }, []);

    return needsAuthorizaiton;
}
