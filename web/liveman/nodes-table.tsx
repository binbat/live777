import { useRefreshTimer } from '../shared/hooks/use-refresh-timer';
import { StyledCheckbox } from '../shared/components/styled-checkbox';

import { getNodes } from './api';

export function NodesTable() {
    const nodes = useRefreshTimer([], getNodes);

    return (
        <>
            <fieldset>
                <legend class="inline-flex items-center">
                    <span>Nodes (total: {nodes.data.length})</span>
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
                                <td class="text-center"><a href={n.url} target="blank">{n.url}</a></td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </fieldset>
        </>
    );
}
