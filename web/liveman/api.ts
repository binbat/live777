import wretch from 'wretch';

import type { Stream } from '../shared/api';
import { makeAuthorizationMiddleware } from '../shared/authorization-middleware';

const authMiddleware = makeAuthorizationMiddleware();

const w = wretch().middlewares([authMiddleware]);

export const setAuthToken = authMiddleware.setAuthorization;
export const addUnauthorizedCallback = authMiddleware.addUnauthorizedCallback;
export const removeUnauthorizedCallback = authMiddleware.removeUnauthorizedCallback;

export interface LoginResponse {
    token_type: string;
    access_token: string;
}

export function login(username: string, password: string) {
    return w.url('/api/login').post({ username, password }).json<LoginResponse>();
}

export interface Node {
    alias: string;
    url: string;
    duration: String;
    strategy: any,
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
    return w.url('/api/token').post(req).json<StreamTokenResponse>();
}
