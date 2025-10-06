import wretch from 'wretch';
import QueryStringAddon from 'wretch/addons/queryString';

import { type Stream } from '../shared/api';
import { makeAuthorizationMiddleware } from '../shared/authorization-middleware';

const authMiddleware = makeAuthorizationMiddleware();

const w = wretch().addon(QueryStringAddon).middlewares([authMiddleware]);

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
    duration: string;
    strategy?: Record<string, string | number | boolean>,
    status: 'running' | 'stopped';
}

export function getNodes() {
    return w.url('/api/nodes/').get().json<Node[]>();
}

export { type Stream };

export function getStreams(nodes?: string[]) {
    return w.url('/api/streams/').query({ nodes }).get().json<Stream[]>();
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

// Recording & Playback APIs
// removed: unused recording streams listing API placeholder

export interface RecordingSession {
    id?: string;
    stream: string;
    start_ts: number;
    end_ts: number | null;
    duration_ms: number | null;
    mpd_path: string;
    status: 'Active' | 'Completed' | 'Failed';
}

export interface RecordingSessionsResponse {
    sessions: RecordingSession[];
    total_count: number;
}

export interface RecordingSessionQuery {
    stream?: string;
    status?: string;
    start_ts?: number;
    end_ts?: number;
    limit?: number;
    offset?: number;
}

export function getSegmentUrl(path: string) {
    // Use encodeURI so that "/" remains as path separators (server expects wildcard path)
    return `/api/record/object/${encodeURI(path)}`;
}

export interface RecordingIndexEntry {
    year: number;
    month: number;
    day: number;
    mpd_path: string;
}

export function getRecordingIndexStreams() {
    return w.url('/api/playback').get().json<string[]>();
}

export function getRecordingIndexByStream(stream: string) {
    return w.url(`/api/playback/${encodeURIComponent(stream)}`).get().json<RecordingIndexEntry[]>();
}
