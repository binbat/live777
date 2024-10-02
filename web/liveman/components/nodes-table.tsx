import { useRefreshTimer } from '../../shared/hooks/use-refresh-timer';
import { StyledCheckbox } from '../../shared/components/styled-checkbox';

import { getNodes } from '../api';

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
                            <th>Max Publish Cnt.</th>
                            <th>Max subscribe Cnt.</th>
                            <th class="min-w-72">API URL</th>
                        </tr>
                    </thead>
                    <tbody>
                        {nodes.data.map(n => (
                            <tr>
                                <td class="text-center">{n.alias}</td>
                                <td class="text-center">{n.status}</td>
                                <td class="text-center">{n.pub_max}</td>
                                <td class="text-center">{n.sub_max}</td>
                                <td class="text-center"><a href={n.url} target="_blank">{n.url}</a></td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </fieldset>
        </>
    );
}
