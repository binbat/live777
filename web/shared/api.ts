import wretch from "wretch";

import { makeAuthorizationMiddleware } from "../shared/authorization-middleware";

const authMiddleware = makeAuthorizationMiddleware();

const w = wretch().middlewares([authMiddleware]);

export const setAuthToken = authMiddleware.setAuthorization;
export const addUnauthorizedCallback = authMiddleware.addUnauthorizedCallback;
export const removeUnauthorizedCallback = authMiddleware.removeUnauthorizedCallback;

export function deleteSession(streamId: string, clientId: string) {
    return w.url(`/session/${streamId}/${clientId}`).delete().res();
}
export function getCurrentNode(): string {
    const urlParams = new URLSearchParams(window.location.search);
    return urlParams.get("nodes") || "0";
}
export async function createStream(streamId: string): Promise<any> {
    const currentNode = getCurrentNode();
    console.log("Current Node:", currentNode);
    console.log("Request URL:", `/api/streams/${streamId}?nodes=${currentNode}`);

    try {
        return await w.url(`/api/streams/${streamId}?nodes=${currentNode}`).post().res();
    } catch (error: unknown) {
        if (error instanceof Object && "response" in error && typeof (error as any).response.status === "number") {
            const wretchError = error as { response: { status: number } };
            if (wretchError.response.status === 409) {
                window.alert("资源已存在，请使用不同的 streamId");
                throw new Error("Resource already exists");
            }
        }
        throw error;
    }
}

export function deleteStream(streamId: string) {
    return w.url(`/api/streams/${streamId}`).delete().res();
}

type SessionConnectionState = "new" | "connecting" | "connected" | "disconnected" | "failed" | "closed";

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
    return w.url("/api/streams/").get().json<Stream[]>();
}

export function cascade(streamId: string, params: Cascade) {
    return w.url(`/api/cascade/${streamId}`).post(params).res();
}
