import { useRefreshTimer } from '../../shared/hooks/use-refresh-timer';
import { StyledCheckbox } from '../../shared/components/styled-checkbox';

import { type Node, getNodes } from '../api';

async function getNodesSorted() {
    const nodes = await getNodes();
    return nodes.sort((a, b) => a.alias.localeCompare(b.alias));
}

export function NodesTable() {
    const nodes = useRefreshTimer([], getNodesSorted);

    return (
        <>
            <fieldset>
                <legend class="inline-flex items-center gap-x-4">
                    <span>Nodes (total: {nodes.data.length})</span>
                    <button onClick={() => nodes.updateData()}>Refresh</button>
                    <StyledCheckbox label="Auto Refresh" checked={nodes.isRefreshing} onClick={nodes.toggleTimer}></StyledCheckbox>
                </legend>
                <table>
                    <thead>
                        <tr>
                            <th class="min-w-24">Alias</th>
                            <th class="min-w-24">Status</th>
                            <th>Delay</th>
                            <th class="min-w-72">Strategy</th>
                            <th class="min-w-72">API URL</th>
                        </tr>
                    </thead>
                    <tbody>
                        {nodes.data.map(n => (
                            <tr>
                                <td class="text-center">{n.alias}</td>
                                <td class="text-center">{n.status}</td>
                                <td class="text-center">{n.duration}</td>
                                <td class="text-center">
                                    { n.strategy
                                        ? <NodeStrategyTable strategy={n.strategy} />
                                        : <>-</>
                                    }
                                </td>
                                <td class="text-center"><a href={n.url} target="_blank">{n.url}</a></td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </fieldset>
        </>
    );
}

type NodeStrategyTableProps = Pick<Node, 'strategy'>;

function NodeStrategyTable({ strategy }: NodeStrategyTableProps) {
    return (
        <div class="h-[1lh] overflow-hidden relative group hover:overflow-visible">
            <table class="mx-auto px-1 rounded group-hover:absolute group-hover:inset-x-0 group-hover:z-1 group-hover:outline group-hover:outline-indigo-500" data-theme="">
                <tbody>
                    {Object.entries(strategy).map(([k, v]) => (
                        <tr>
                            <th class="text-left">{k}</th>
                            <td>{`${v}`}</td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </div>
    );
}
