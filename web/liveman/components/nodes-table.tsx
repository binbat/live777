import { useContext, useEffect } from 'preact/hooks';

import { Badge, Button, Checkbox, Dropdown, Link, Table } from 'react-daisyui';
import { ArrowPathIcon, EllipsisHorizontalIcon } from '@heroicons/react/24/outline';

import { TokenContext } from '@/shared/context';
import { useRefreshTimer } from '@/shared/hooks/use-refresh-timer';

import { type Node, getNodes } from '../api';

async function getNodesSorted() {
    const nodes = await getNodes();
    return nodes.sort((a, b) => a.alias.localeCompare(b.alias));
}

export function NodesTable() {
    const nodes = useRefreshTimer([], getNodesSorted);
    const tokenContext = useContext(TokenContext);

    useEffect(() => {
        if (tokenContext.token.length > 0) {
            nodes.updateData();
        }
    }, [tokenContext.token]);

    return (
        <>
            <div className="flex items-center gap-2 px-4 h-12">
                <span className="font-bold text-lg">Nodes</span>
                <Badge color="ghost" className="font-bold mr-auto">{nodes.data.length}</Badge>
                <Button
                    size="sm"
                    color="ghost"
                    endIcon={<Checkbox size="xs" checked={nodes.isRefreshing} />}
                    onClick={nodes.toggleTimer}
                >Auto Refresh</Button>
                <Button
                    size="sm"
                    color="ghost"
                    endIcon={<ArrowPathIcon className="size-4 stroke-current" />}
                    onClick={nodes.updateData}
                >Refresh</Button>
            </div>

            <Table className="overflow-x-auto">
                <Table.Head>
                    <span>Alias</span>
                    <span>Status</span>
                    <span>Delay</span>
                    <span>Strategy</span>
                    <span>API URL</span>
                </Table.Head>
                <Table.Body>
                    {nodes.data.length > 0 ? nodes.data.map(n =>
                        <Table.Row>
                            <span>{n.alias}</span>
                            <span>{n.status}</span>
                            <span>{n.duration}</span>
                            <NodeStrategyLabel strategy={n.strategy} />
                            <Link href={location.href + '?nodes=' + n.alias} target="_blank">{location.href + '?nodes=' + n.alias}</Link>
                        </Table.Row>
                    ) : <tr><td colspan={5} className="text-center">N/A</td></tr>}
                </Table.Body>
            </Table>
        </>
    );
}

type NodeStrategyLabelProps = Pick<Node, 'strategy'>;

function NodeStrategyLabel({ strategy }: NodeStrategyLabelProps) {
    const entries = Object.entries(strategy ?? {});
    if (entries.length <= 1) {
        return <span className="font-mono">{entries[0]?.join(' = ') ?? '-'}</span>;
    }
    return (
        <Dropdown hover>
            <Dropdown.Toggle button={false} className="font-mono flex items-center gap-1">
                {/* <InformationCircleIcon className="size-4" /> */}
                <span>{entries[0].join(' = ')}</span>
                <EllipsisHorizontalIcon className="size-4" />
            </Dropdown.Toggle>
            <Dropdown.Menu className="z-10 mx-[-1rem]">
                <Table size="xs">
                    <Table.Body>
                        {entries.map(([k, v]) =>
                            <Table.Row>
                                <span className="text-sm font-mono">{`${k}`}</span>
                                <span className="text-sm font-mono">{`${v}`}</span>
                            </Table.Row>
                        )}
                    </Table.Body>
                </Table>
            </Dropdown.Menu>
        </Dropdown>
    );
}
