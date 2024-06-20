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

export async function allStream(): Promise<Stream[]> {
    return (await fetch("/api/streams/")).json()
}

export async function reforward(streamId: string, url: string): Promise<void> {
    fetch(`/admin/reforward/${streamId}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
        },
        body: JSON.stringify({
            targetUrl: url,
        }),
    })
}
