import { Stream } from '../shared/api';

export interface Node {
    alias: string;
    url: string;
    pub_max: number;
    sub_max: number;
    status: 'running' | 'stopped';
}

export async function getNodes(): Promise<Node[]> {
    return (await fetch('/api/nodes/')).json();
}


export interface StreamDetail {
    [key: string]: Stream;
}

export async function getStreamDetail(streamId: string): Promise<StreamDetail> {
    return (await fetch(`/api/streams/${streamId}`)).json();
}
