import wretch from 'wretch';

const base = wretch();

let w = base;

export function setAuthToken(token: string) {
    w = base.auth(token);
}

export function deleteSession(streamId: string, clientId: string) {
    return w.url(`/session/${streamId}/${clientId}`).delete();
}

export function createStream(streamId: string) {
    return w.url(`/api/streams/${streamId}`).post();
}

export function deleteStream(streamId: string) {
    return w.url(`/api/streams/${streamId}`).delete();
}

type SessionConnectionState =
    'new' |
    'connecting' |
    'connected' |
    'disconnected' |
    'failed' |
    'closed'

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
    return w.url(`/api/cascade/${streamId}`).post(params);
}
