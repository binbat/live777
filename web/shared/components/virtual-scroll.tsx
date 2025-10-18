import { useMemo, useRef, useState } from 'preact/hooks';

interface VirtualScrollProps<T> {
    items: T[];
    itemHeight: number;
    containerHeight: number;
    renderItem: (item: T, index: number) => preact.ComponentChild;
    overscan?: number;
}

export function VirtualScroll<T>({ 
    items, 
    itemHeight, 
    containerHeight,
    renderItem,
    overscan = 5 
}: VirtualScrollProps<T>) {
    const [scrollTop, setScrollTop] = useState(0);
    const scrollElementRef = useRef<HTMLDivElement>(null);

    const { visibleItems, totalHeight, offsetY } = useMemo(() => {
        const startIndex = Math.floor(scrollTop / itemHeight);
        const endIndex = Math.min(
            startIndex + Math.ceil(containerHeight / itemHeight) + overscan,
            items.length
        );

        const visibleStartIndex = Math.max(0, startIndex - overscan);
        const visibleItems = items.slice(visibleStartIndex, endIndex);
        
        return {
            visibleItems: visibleItems.map((item, index) => ({
                item,
                index: visibleStartIndex + index
            })),
            totalHeight: items.length * itemHeight,
            offsetY: visibleStartIndex * itemHeight
        };
    }, [items, itemHeight, scrollTop, containerHeight, overscan]);

    const handleScroll = (event: Event) => {
        const target = event.target as HTMLDivElement;
        setScrollTop(target.scrollTop);
    };

    return (
        <div
            ref={scrollElementRef}
            onScroll={handleScroll}
            style={{ height: containerHeight, overflow: 'auto' }}
        >
            <div style={{ height: totalHeight, position: 'relative' }}>
                <div style={{ transform: `translateY(${offsetY}px)` }}>
                    {visibleItems.map(({ item, index }) => (
                        <div key={index} style={{ height: itemHeight }}>
                            {renderItem(item, index)}
                        </div>
                    ))}
                </div>
            </div>
        </div>
    );
}