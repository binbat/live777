import { useRef, useState } from 'preact/hooks';

const formatTimestamp: (ts: number) => string = ts => {
    const ms = (ts % 1000).toString(10).padStart(3, '0');
    let t = Math.trunc(ts / 1000);
    const s = (t % 60).toString(10).padStart(2, '0');
    t = Math.trunc(t / 60);
    const m = (t % 60).toString(10).padStart(2, '0');
    t = Math.trunc(t / 60);
    const h = t % 60;
    if (h === 0) {
        return `${m}:${s}.${ms}`;
    } else {
        return `${h}:${m}:${s}.${ms}`;
    }
};

export function useLogger() {
    const refFirstTimestamp = useRef<number>(NaN);
    const [logs, setLogs] = useState<string[]>([]);
    const clear = () => {
        refFirstTimestamp.current = NaN;
        setLogs([]);
    };
    const log = (text: string, time = Date.now()) => {
        const t = refFirstTimestamp.current;
        let diff = 0;
        if (Number.isNaN(t)) {
            refFirstTimestamp.current = time;
        } else {
            diff = time - t;
        }
        const line = `${formatTimestamp(diff)} ${text}`;
        setLogs(l => [...l, line]);
    };
    return {
        logs, log, clear
    };
}
