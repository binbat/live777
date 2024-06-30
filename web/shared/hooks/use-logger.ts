import { useRef, useState } from 'preact/hooks'

export function useLogger() {
    const refLastTimestamp = useRef<number>(Infinity)
    const [logs, setLogs] = useState<string[]>([])
    const clear = () => {
        refLastTimestamp.current = Infinity
        setLogs([])
    }
    const log = (text: string, time = Date.now()) => {
        const t = refLastTimestamp.current
        const diff = time > t ? time - t : 0
        const line = `${text} (${diff} ms)`
        refLastTimestamp.current = time
        setLogs(l => [...l, line])
    }
    return {
        logs, log, clear
    }
}
