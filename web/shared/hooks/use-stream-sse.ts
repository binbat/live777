import { useCallback, useEffect, useRef, useState } from 'preact/hooks';

export interface StreamSSEEvent<T> {
    data: T;
}

export interface UseStreamSSEOptions<T> {
    url: string | null;
    token: string;
    parse: (data: string) => T;
    enabled?: boolean;
}

export interface UseStreamSSEReturn<T> {
    data: T;
    connected: boolean;
    /** Reconnecting after having connected at least once (clean close, in backoff). */
    reconnecting: boolean;
    error: Error | null;
    reconnect: () => void;
}

interface SSEMessage {
    data: string;
}

const INITIAL_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 30000;

function parseSSEBuffer(buffer: string): { messages: SSEMessage[]; remaining: string } {
    const parts = buffer.split('\n\n');
    const remaining = parts.pop() ?? '';
    const messages: SSEMessage[] = [];

    for (const part of parts) {
        if (!part.trim()) {
            continue;
        }

        const lines = part.split('\n');
        let data = '';
        for (const line of lines) {
            if (line.startsWith('data:')) {
                const value = line.slice(5).trimStart();
                data = data ? `${data}\n${value}` : value;
            }
        }
        if (data) {
            messages.push({ data });
        }
    }

    return { messages, remaining };
}

export function useStreamSSE<T>(
    options: UseStreamSSEOptions<T>,
    initialData: T,
): UseStreamSSEReturn<T> {
    const { url, token, parse, enabled = true } = options;
    const [data, setData] = useState<T>(initialData);
    const [connected, setConnected] = useState(false);
    const [hasConnectedOnce, setHasConnectedOnce] = useState(false);
    const [error, setError] = useState<Error | null>(null);
    const [retryCount, setRetryCount] = useState(0);
    const reconnect = useCallback(() => setRetryCount(c => c + 1), []);
    const abortControllerRef = useRef<AbortController | null>(null);

    useEffect(() => {
        if (!url || !enabled) {
            setConnected(false);
            return;
        }

        let disposed = false;
        let retryTimeout = 0;
        let backoffMs = INITIAL_BACKOFF_MS;

        const connect = async () => {
            abortControllerRef.current?.abort();
            const abortController = new AbortController();
            abortControllerRef.current = abortController;

            try {
                setError(null);

                const headers: Record<string, string> = {
                    Accept: 'text/event-stream',
                };
                if (token) {
                    headers.Authorization = token.includes(' ') ? token : `Bearer ${token}`;
                }

                const response = await fetch(url, {
                    method: 'GET',
                    headers,
                    signal: abortController.signal,
                });

                if (!response.ok) {
                    throw new Error(`SSE request failed (HTTP ${response.status})`);
                }

                if (!response.body) {
                    throw new Error('SSE response body is empty');
                }

                setConnected(true);
                setHasConnectedOnce(true);
                backoffMs = INITIAL_BACKOFF_MS;

                const reader = response.body.getReader();
                const decoder = new TextDecoder();
                let buffer = '';

                while (!disposed) {
                    const { done, value } = await reader.read();
                    if (done) {
                        break;
                    }

                    buffer += decoder.decode(value, { stream: true });
                    const { messages, remaining } = parseSSEBuffer(buffer);
                    buffer = remaining;

                    for (const message of messages) {
                        try {
                            setData(parse(message.data));
                        } catch (err) {
                            console.error('Failed to parse SSE message data:', err);
                        }
                    }
                }

                if (!disposed) {
                    setConnected(false);
                    retryTimeout = window.setTimeout(() => {
                        reconnect();
                    }, backoffMs);
                    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
                }
            } catch (err) {
                if (disposed || abortController.signal.aborted) {
                    return;
                }
                setConnected(false);
                setError(err instanceof Error ? err : new Error(String(err)));
                retryTimeout = window.setTimeout(() => {
                    reconnect();
                }, backoffMs);
                backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
            }
        };

        connect();

        return () => {
            disposed = true;
            window.clearTimeout(retryTimeout);
            abortControllerRef.current?.abort();
            abortControllerRef.current = null;
        };
    }, [url, token, enabled, retryCount, parse, reconnect]);

    return {
        data,
        connected,
        reconnecting: !connected && !error && hasConnectedOnce,
        error,
        reconnect,
    };
}
