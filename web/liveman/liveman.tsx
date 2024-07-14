import { Live777Logo } from '../shared/components/live777-logo';
import { NodesTable } from './nodes-table';
import { StreamsTable } from '../shared/components/streams-table';

export function Liveman() {
    return (
        <>
            <Live777Logo />
            <NodesTable />
            <StreamsTable cascade={false} />
        </>
    );
}
