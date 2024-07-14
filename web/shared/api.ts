export async function deleteSession(streamId: string, clientId: string) {
    return fetch(`/session/${streamId}/${clientId}`, {
        method: 'DELETE',
    });
}

export async function createStream(streamId: string) {
    return fetch(`/api/streams/${streamId}`, {
        method: 'POST',
    });
}

export async function deleteStream(streamId: string) {
    return fetch(`/api/streams/${streamId}`, {
        method: 'DELETE',
    });
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

export async function getStreams(): Promise<Stream[]> {
    return (await fetch('/api/streams/')).json();
}

export async function cascade(streamId: string, params: Cascade): Promise<void> {
    fetch(`/api/cascade/${streamId}`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify(params),
    });
}
