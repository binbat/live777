import { useEffect, useState } from 'preact/hooks';

export function useRefreshTimer<T>(initial: T, fetchContent: () => Promise<T>, timeout = 3000, immediate = true): [T, boolean, () => void] {
    const [content, setContent] = useState<T>(initial)
    const [isImmediate, _setIsImmediate] = useState(immediate)
    const [refreshTimer, setRefreshTimer] = useState(-1)
    const isRefreshing = refreshTimer > 0
    const updateContent = async () => {
        setContent(await fetchContent())
    }
    useEffect(() => {
        if (isImmediate) {
            updateContent()
        }
        return () => {
            if (isRefreshing) {
                window.clearInterval(refreshTimer)
            }
        }
    }, [])
    useEffect(() => {
        if (isRefreshing) {
            clearInterval(refreshTimer)
            setRefreshTimer(window.setInterval(updateContent, timeout))
        }
    }, [timeout])
    const toggleTimer = () => {
        if (isRefreshing) {
            clearInterval(refreshTimer)
            setRefreshTimer(-1)
        } else {
            updateContent()
            setRefreshTimer(window.setInterval(updateContent, timeout))
        }
    }
    return [content, isRefreshing, toggleTimer]
}
