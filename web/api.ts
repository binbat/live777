export async function delStream(streamId: string, clientId: string) {
    return fetch(`/resource/${streamId}/${clientId}`, {
        method: "DELETE",
    })
}

type SessionConnectionState =
    'Unspecified' |
    'new' |
    'connecting' |
    'connected' |
    'disconnected' |
    'failed' |
    'closed'

export interface StreamInfo {
    id: string;
    createTime: number;
    publishLeaveTime: number;
    subscribeLeaveTime: number;
    publishSessionInfo: {
        id: string;
        createTime: number;
        connectState: SessionConnectionState;
    };
    subscribeSessionInfos: SubscribeSessionInfo[];
}

export interface SubscribeSessionInfo {
    id: string;
    createTime: number;
    connectState: SessionConnectionState;
    reforward?: {
        targetUrl: string;
        resourceUrl: string;
    };
}

export async function allStream(): Promise<StreamInfo[]> {
    return (await fetch("/admin/infos")).json()
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
