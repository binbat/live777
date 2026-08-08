import {
    computed,
    ref,
    shallowRef,
    toValue,
    watchEffect,
    type ComputedRef,
    type MaybeRefOrGetter,
    type Ref,
    type ShallowRef,
} from "vue";

export interface StreamSSEEvent<T> {
    data: T;
}

export interface UseStreamSSEOptions<T> {
    url: MaybeRefOrGetter<string | null>;
    token: MaybeRefOrGetter<string>;
    parse: (data: string) => T;
    enabled?: MaybeRefOrGetter<boolean>;
}

export interface UseStreamSSEReturn<T> {
    data: ShallowRef<T>;
    connected: Ref<boolean>;
    /** Reconnecting after having connected at least once (clean close, in backoff). */
    reconnecting: ComputedRef<boolean>;
    error: ShallowRef<Error | null>;
    reconnect: () => void;
}

interface SSEMessage {
    data: string;
}

const INITIAL_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 30000;

function parseSSEBuffer(buffer: string): { messages: SSEMessage[]; remaining: string } {
    const parts = buffer.split("\n\n");
    const remaining = parts.pop() ?? "";
    const messages: SSEMessage[] = [];

    for (const part of parts) {
        if (!part.trim()) {
            continue;
        }

        const lines = part.split("\n");
        let data = "";
        for (const line of lines) {
            if (line.startsWith("data:")) {
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
    const { parse } = options;
    const data = shallowRef(initialData) as ShallowRef<T>;
    const connected = ref(false);
    const hasConnectedOnce = ref(false);
    const error = shallowRef<Error | null>(null);
    const retryCount = ref(0);
    const reconnect = () => {
        retryCount.value += 1;
    };
    let abortController: AbortController | null = null;

    watchEffect((onCleanup) => {
        // Read every reactive input synchronously so the effect tracks it;
        // retryCount is only tracked to re-run on manual reconnects.
        const url = toValue(options.url);
        const token = toValue(options.token);
        const enabled = toValue(options.enabled ?? true);
        void retryCount.value;

        if (!url || !enabled) {
            connected.value = false;
            return;
        }

        let disposed = false;
        let retryTimeout = 0;
        let backoffMs = INITIAL_BACKOFF_MS;

        const connect = async () => {
            abortController?.abort();
            const controller = new AbortController();
            abortController = controller;

            try {
                error.value = null;

                const headers: Record<string, string> = {
                    Accept: "text/event-stream",
                };
                if (token) {
                    headers.Authorization = token.includes(" ") ? token : `Bearer ${token}`;
                }

                const response = await fetch(url, {
                    method: "GET",
                    headers,
                    signal: controller.signal,
                });

                if (!response.ok) {
                    throw new Error(`SSE request failed (HTTP ${response.status})`);
                }

                if (!response.body) {
                    throw new Error("SSE response body is empty");
                }

                connected.value = true;
                hasConnectedOnce.value = true;
                backoffMs = INITIAL_BACKOFF_MS;

                const reader = response.body.getReader();
                const decoder = new TextDecoder();
                let buffer = "";

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
                            data.value = parse(message.data);
                        } catch (err) {
                            console.error("Failed to parse SSE message data:", err);
                        }
                    }
                }

                if (!disposed) {
                    connected.value = false;
                    retryTimeout = window.setTimeout(() => {
                        reconnect();
                    }, backoffMs);
                    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
                }
            } catch (err) {
                if (disposed || controller.signal.aborted) {
                    return;
                }
                connected.value = false;
                error.value = err instanceof Error ? err : new Error(String(err));
                retryTimeout = window.setTimeout(() => {
                    reconnect();
                }, backoffMs);
                backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
            }
        };

        void connect();

        onCleanup(() => {
            disposed = true;
            window.clearTimeout(retryTimeout);
            abortController?.abort();
            abortController = null;
        });
    });

    return {
        data,
        connected,
        reconnecting: computed(() => !connected.value && !error.value && hasConnectedOnce.value),
        error,
        reconnect,
    };
}
