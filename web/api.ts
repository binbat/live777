export async function delStream(streamId: string, clientId: string) {
    return fetch(`/session/${streamId}/${clientId}`, {
        method: "DELETE",
    })
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
    reforward?: {
        targetUrl: string;
        resourceUrl: string;
    };
}

export interface Cascade{
    token?: string;
    src?: string;
    dst?: string;
}

export async function allStream(): Promise<Stream[]> {
    return (await fetch("/api/streams/")).json()
}

export async function cascade(streamId: string, params: Cascade): Promise<void> {
    fetch(`/api/cascade/${streamId}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
        },
        body: JSON.stringify(params),
    })
}
