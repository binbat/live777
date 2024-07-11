import { useEffect, useState } from 'preact/hooks';

export function useRefreshTimer<T>(initial: T, fetchData: () => Promise<T>, timeout = 3000, immediate = true) {
    const [data, setData] = useState<T>(initial)
    const [isImmediate, _setIsImmediate] = useState(immediate)
    const [refreshTimer, setRefreshTimer] = useState(-1)
    const isRefreshing = refreshTimer > 0
    const updateData = async () => {
        setData(await fetchData())
    }
    useEffect(() => {
        if (isImmediate) {
            updateData()
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
            setRefreshTimer(window.setInterval(updateData, timeout))
        }
    }, [timeout])
    const toggleTimer = () => {
        if (isRefreshing) {
            clearInterval(refreshTimer)
            setRefreshTimer(-1)
        } else {
            updateData()
            setRefreshTimer(window.setInterval(updateData, timeout))
        }
    }
    return {
        data,
        isRefreshing,
        updateData,
        toggleTimer
    }
}
