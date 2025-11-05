import { type Accessor, createSignal } from "solid-js";

const formatTimestamp: (ts: number) => string = (ts) => {
    const ms = (ts % 1000).toString(10).padStart(3, "0");
    let t = Math.trunc(ts / 1000);
    const s = (t % 60).toString(10).padStart(2, "0");
    t = Math.trunc(t / 60);
    const m = (t % 60).toString(10).padStart(2, "0");
    t = Math.trunc(t / 60);
    const h = t % 60;
    if (h === 0) {
        return `${m}:${s}.${ms}`;
    } else {
        return `${h}:${m}:${s}.${ms}`;
    }
};

export function createLogger(): [
    Accessor<string[]>,
    (text: string, time?: number) => void,
    () => void,
] {
    const [logs, setLogs] = createSignal<string[]>([]);
    let firstTimestamp: number | undefined;

    const clear = () => {
        firstTimestamp = undefined;
        setLogs([]);
    };

    const log = (text: string, time = Date.now()) => {
        let diff = 0;
        if (firstTimestamp === undefined) {
            firstTimestamp = time;
        } else {
            diff = time - firstTimestamp;
        }
        const line = `${formatTimestamp(diff)} ${text}`;
        setLogs((l) => [...l, line]);
    };

    return [logs, log, clear];
}
