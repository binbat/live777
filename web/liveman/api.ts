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
export interface RecordingStreamsResponse {
    streams: string[];
}

export function getRecordingStreams() {
    return w.url('/api/record/streams').get().json<RecordingStreamsResponse>();
}

export interface Segment {
    id: string;
    start_ts: number;
    end_ts: number;
    duration_ms: number;
    path: string;
    is_keyframe: boolean;
    created_at: string;
}

export interface TimelineResponse {
    stream: string;
    total_count: number;
    segments: Segment[];
}

export interface TimelineQuery {
    start_ts?: number;
    end_ts?: number;
    limit?: number;
    offset?: number;
}

export function getTimeline(stream: string, query?: TimelineQuery) {
    return w.url(`/api/record/${stream}/timeline`).query(query).get().json<TimelineResponse>();
}

export function getMPD(stream: string, query?: { start_ts?: number; end_ts?: number }) {
    return w.url(`/api/record/${stream}/mpd`).query(query).get().text();
}

export function getSegmentUrl(path: string) {
    return `/api/record/object/${encodeURIComponent(path)}`;
}
