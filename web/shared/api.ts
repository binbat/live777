import wretch from 'wretch';

import { makeAuthorizationMiddleware } from '../shared/authorization-middleware';

const authMiddleware = makeAuthorizationMiddleware();

const w = wretch().middlewares([authMiddleware]);

export const setAuthToken = authMiddleware.setAuthorization;
export const addUnauthorizedCallback = authMiddleware.addUnauthorizedCallback;
export const removeUnauthorizedCallback = authMiddleware.removeUnauthorizedCallback;

export function deleteSession(streamId: string, clientId: string) {
    return w.url(`/session/${streamId}/${clientId}`).delete().res();
}

export async function createStream(streamId: string, nodes: string | null = null): Promise<unknown> {
    let url = `/api/streams/${streamId}`;
    if (nodes !== null) {
        url += `?nodes=${nodes}`;
    }
    return w.url(url).post().res();
}

export function deleteStream(streamId: string) {
    return w.url(`/api/streams/${streamId}`).delete().res();
}

type SessionConnectionState = 'new' | 'connecting' | 'connected' | 'disconnected' | 'failed' | 'closed';

export interface Stream {
    id: string;
    createdAt: number;
    publish: {
        leaveAt: number;
        sessions: Session[];
    };
    subscribe: {
        leaveAt: number;
        sessions: Session[];
    };
}

export interface Session {
    id: string;
    createdAt: number;
    state: SessionConnectionState;
    cascade?: {
        sourceUrl?: string;
        targetUrl?: string;
        sessionUrl: string;
    };
    reforward?: boolean;
}

export interface Cascade {
    token?: string;
    sourceUrl?: string;
    targetUrl?: string;
}

export function getStreams() {
    return w.url('/api/streams/').get().json<Stream[]>();
}

export function cascade(streamId: string, params: Cascade) {
    return w.url(`/api/cascade/${streamId}`).post(params).res();
}

export function startRecording(streamId: string) {
    return w.url(`/api/record/${streamId}`).post().res();
}
