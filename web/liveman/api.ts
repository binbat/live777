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

export function setAuthToken(token: string) {
    w = base.auth(token);
}

export interface LoginResponse {
    token_type: string;
    access_token: string;
}

export function login(username: string, password: string) {
    return w.url('/login').post({ username, password }).json<LoginResponse>();
}

export interface Node {
    alias: string;
    url: string;
    pub_max: number;
    sub_max: number;
    status: 'running' | 'stopped';
}

export function getNodes() {
    return w.url('/api/nodes/').get().json<Node[]>();
}

export interface StreamDetail {
    [key: string]: Stream;
}

export function getStreamDetail(streamId: string) {
    return w.url(`/api/streams/${streamId}`).get().json<StreamDetail>();
}

export interface CreateStreamTokenRequest {
    /**
     * stream id, use `*` match any stream id
     */
    id: string;
    /**
     * Validity duration (second)
     */
    duration: number;
    /**
     * can use whep
     */
    subscribe: boolean;
    /**
     * can use whip
     */
    publish: boolean;
    /**
     * can use cascade and delete stream
     */
    admin: boolean;
}

export interface StreamTokenResponse {
    token_type: string;
    access_token: string;
}

export function createStreamToken(req: CreateStreamTokenRequest) {
    return w.url('/token').post(req).json<StreamTokenResponse>();
}
