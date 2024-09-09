import wretch from 'wretch';

import type { Stream } from '../shared/api';

const unauthorizedCallbacks: (() => void)[] = [];

export function addUnauthorizedCallback(cb: () => void) {
    unauthorizedCallbacks.push(cb);
}

export function removeUnauthorizedCallback(cb: () => void) {
    const i = unauthorizedCallbacks.indexOf(cb);
    if (i >= 0) {
        unauthorizedCallbacks.splice(i, 1);
    }
}

const base = wretch().middlewares([
    (next) => async (url, opts) => {
        const res = await next(url, opts);
        if (res.status === 401) {
            unauthorizedCallbacks.forEach(cb => cb());
        }
        return res;
    }
]);

let w = base;

function setAuthToken(token: string) {
    w = base.auth(token);
}

export interface LoginResponse {
    token_type: string;
    access_token: string;
}

export async function login(username: string, password: string) {
    const res = await w.url('/login').post({ username, password }).json<LoginResponse>();
    setAuthToken(`${res.token_type} ${res.access_token}`);
}

export interface Node {
    alias: string;
    url: string;
    pub_max: number;
    sub_max: number;
    status: 'running' | 'stopped';
}

export function getNodes(): Promise<Node[]> {
    return w.url('/api/nodes/').get().json();
}


export interface StreamDetail {
    [key: string]: Stream;
}

export function getStreamDetail(streamId: string): Promise<StreamDetail> {
    return w.url(`/api/streams/${streamId}`).get().json();
}
