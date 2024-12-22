import { useCallback, useEffect, useState } from 'preact/hooks';

export function useRefreshTimer<T>(initial: T, fetchData: () => Promise<T>, timeout = 3000, immediate = true) {
    const [data, setData] = useState<T>(initial);
    const [isImmediate, _setIsImmediate] = useState(immediate);
    const [refreshTimer, setRefreshTimer] = useState(-1);
    const isRefreshing = refreshTimer > 0;
    const updateData = useCallback(async () => setData(await fetchData()), [fetchData]);
    useEffect(() => {
        if (isImmediate) {
            updateData();
        }
    }, []);
    useEffect(() => {
        if (isRefreshing) {
            window.clearInterval(refreshTimer);
            setRefreshTimer(window.setInterval(updateData, timeout));
        }
        return () => {
            window.clearInterval(refreshTimer);
        };
    }, [updateData, timeout]);
    const toggleTimer = () => {
        if (isRefreshing) {
            clearInterval(refreshTimer);
            setRefreshTimer(-1);
        } else {
            updateData();
            setRefreshTimer(window.setInterval(updateData, timeout));
        }
    };
    return {
        data,
        isRefreshing,
        updateData,
        toggleTimer
    };
}
