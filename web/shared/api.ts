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

const recordUrl = (streamId: string) => `/api/record/${encodeURIComponent(streamId)}`;

export interface StartRecordingOptions {
    base_dir?: string | null;
}

export function startRecording(streamId: string, options: StartRecordingOptions = {}) {
    const payload: StartRecordingOptions = {
        ...options,
    };

    return w
        .url(recordUrl(streamId))
        .post(payload)
        .json<{ id: string; mpd_path: string }>();
}

export async function getRecordingStatus(streamId: string): Promise<boolean> {
    const { recording } = await w.url(recordUrl(streamId)).get().json<{ recording: boolean }>();
    return recording;
}

export async function stopRecording(streamId: string): Promise<boolean> {
    const response = await w.url(recordUrl(streamId)).delete().res();
    if (!response.ok) {
        throw new Error(`Failed to stop recording (HTTP ${response.status})`);
    }

    const body = await response.text();
    if (!body.trim()) {
        return true;
    }

    try {
        const parsed = JSON.parse(body) as { stopped?: boolean };
        if (typeof parsed.stopped === 'boolean') {
            return parsed.stopped;
        }
    } catch {
        // fall through to default true
    }

    return true;
}

export type CapabilityProbeStatus = 'available' | 'unavailable' | 'unauthorized';

let recorderProbeCache: CapabilityProbeStatus | null = null;

export async function probeRecorderFeature(force = false): Promise<CapabilityProbeStatus> {
    if (!force && recorderProbeCache && recorderProbeCache !== 'unauthorized') {
        return recorderProbeCache;
    }

    try {
        const response = await w.url(recordUrl('__feature_probe__')).get().res();
        if (response.status === 401 || response.status === 403) {
            return 'unauthorized';
        }

        const status: CapabilityProbeStatus = response.ok ? 'available' : 'unavailable';
        recorderProbeCache = status;
        return status;
    } catch {
        recorderProbeCache = 'unavailable';
        return 'unavailable';
    }
}