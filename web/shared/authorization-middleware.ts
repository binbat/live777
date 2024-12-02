import { ConfiguredMiddleware } from 'wretch';

const Authorization = 'Authorization';

export type UnauthorizedCallback = () => void;

export interface AuthorizationCallbacks {
    setAuthorization: (token: string) => void;
    addUnauthorizedCallback: (cb: UnauthorizedCallback) => void;
    removeUnauthorizedCallback: (cb: UnauthorizedCallback) => boolean;
}

interface AuthorizationContext {
    token: string | null;
    callbacks: UnauthorizedCallback[];
}

type AuthorizationMiddleware = ConfiguredMiddleware & AuthorizationCallbacks;

export function makeAuthorizationMiddleware(): AuthorizationMiddleware {
    const ctx: AuthorizationContext = {
        token: null,
        callbacks: []
    };
    const middleware: AuthorizationMiddleware = (next) => async (url, opts) => {
        if (ctx.token) {
            if (typeof opts.headers !== 'object') {
                opts.headers = { [Authorization]: ctx.token };
            } else {
                Reflect.set(opts.headers, Authorization, ctx.token);
            }
        }
        const res = await next(url, opts);
        if (res.status === 401) {
            ctx.callbacks.forEach(cb => cb());
        }
        return res;
    };
    middleware.setAuthorization = token => ctx.token = token;
    middleware.addUnauthorizedCallback = cb => ctx.callbacks.push(cb);
    middleware.removeUnauthorizedCallback = cb => {
        const i = ctx.callbacks.indexOf(cb);
        if (i >= 0) {
            ctx.callbacks.splice(i, 1);
            return true;
        }
        return false;
    };
    return middleware;
};
