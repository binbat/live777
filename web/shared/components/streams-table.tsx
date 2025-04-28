import { useState, useRef, useEffect, useContext } from 'preact/hooks';
import { type ReactNode } from 'preact/compat';

import { Badge, Button, Checkbox, Table } from 'react-daisyui';
import { ArrowPathIcon, ArrowRightEndOnRectangleIcon, PlusIcon } from '@heroicons/react/24/outline';

import { type Stream, getStreams, deleteStream } from '../api';
import { formatTime, nextSeqId } from '../utils';
import { useRefreshTimer } from '../hooks/use-refresh-timer';
import { TokenContext } from '../context';

import { type IClientsDialog, ClientsDialog } from './dialog-clients';
import { type ICascadeDialog, CascadePullDialog, CascadePushDialog } from './dialog-cascade';
import { type IPreviewDialog, PreviewDialog } from './dialog-preview';
import { type IWebStreamDialog, WebStreamDialog } from './dialog-web-stream';
import { type INewStreamDialog, NewStreamDialog } from './dialog-new-stream';

async function getStreamsSorted(): Promise<Stream[]> {
    try {
        const streams = await getStreams();
        return streams.sort((a, b) => a.createdAt - b.createdAt);
    } catch {
        return [];
    }
}

export interface StreamTableProps {
    getStreams?: () => Promise<Stream[]>;
    getWhepUrl?: (streamId: string) => string;
    getWhipUrl?: (streamId: string) => string;
    showCascade?: boolean;
    renderExtraActions?: (s: Stream) => ReactNode;
}

export function StreamsTable(props: StreamTableProps) {
    const streams = useRefreshTimer([], props.getStreams ?? getStreamsSorted);
    const [selectedStreamId, setSelectedStreamId] = useState('');
    const refCascadePull = useRef<ICascadeDialog>(null);
    const refCascadePush = useRef<ICascadeDialog>(null);
    const refClients = useRef<IClientsDialog>(null);
    const refNewStream = useRef<INewStreamDialog>(null);
    const [webStreams, setWebStreams] = useState<string[]>([]);
    const [newStreamId, setNewStreamId] = useState('');
    const refWebStreams = useRef<Map<string, IWebStreamDialog>>(new Map());
    const [previewStreams, setPreviewStreams] = useState<string[]>([]);
    const [previewStreamId, setPreviewStreamId] = useState('');
    const refPreviewStreams = useRef<Map<string, IPreviewDialog>>(new Map());
    const tokenContext = useContext(TokenContext);

    useEffect(() => {
        streams.updateData();
    }, [tokenContext.token]);

    const handleViewClients = (id: string) => {
        setSelectedStreamId(id);
        refClients.current?.show();
    };

    const handleCascadePullStream = () => {
        const newStreamId = nextSeqId('pull-', streams.data.map(s => s.id));
        refCascadePull.current?.show(newStreamId);
    };

    const handleCascadePushStream = (id: string) => {
        refCascadePush.current?.show(id);
    };

    const handlePreview = (id: string) => {
        if (previewStreams.includes(id)) {
            refPreviewStreams.current.get(id)?.show(id);
        } else {
            setPreviewStreams([...previewStreams, id]);
            setPreviewStreamId(id);
        }
    };

    useEffect(() => {
        refPreviewStreams.current.get(previewStreamId)?.show(previewStreamId);
    }, [previewStreamId]);

    const handlePreviewStop = (id: string) => {
        setPreviewStreamId('');
        setPreviewStreams(previewStreams.filter(s => s !== id));
    };

    const handleNewStream = () => {
        const newStreamId = nextSeqId('web-', webStreams.concat(streams.data.map(s => s.id)));
        refNewStream.current?.show(newStreamId);
    };

    const handleNewStreamId = (id: string) => {
        setWebStreams([...webStreams, id]);
        setNewStreamId(id);
    };

    useEffect(() => {
        refWebStreams.current.get(newStreamId)?.show(newStreamId);
    }, [newStreamId]);

    const handleOpenWebStream = (id: string) => {
        refWebStreams.current.get(id)?.show(id);
    };

    const handleWebStreamStop = (id: string) => {
        setNewStreamId('');
        setWebStreams(webStreams.filter(s => s !== id));
    };

    const handleOpenPlayerPage = (id: string) => {
        const params = new URLSearchParams();
        params.set('id', id);
        params.set('autoplay', '');
        params.set('muted', '');
        params.set('reconnect', '3000');
        params.set('token', tokenContext.token);
        const url = new URL(`/tools/player.html?${params.toString()}`, location.origin);
        window.open(url);
    };

    const handleOpenDebuggerPage = (id: string) => {
        const params = new URLSearchParams();
        params.set('id', id);
        params.set('token', tokenContext.token);
        const url = new URL(`/tools/debugger.html?${params.toString()}`, location.origin);
        window.open(url);
    };

    const handleDestroyStream = async (id: string) => {
        await deleteStream(id);
        await streams.updateData();
    };

    return (
        <>
            <div className="flex items-center gap-2 px-4 h-12">
                <span className="font-bold text-lg">Streams</span>
                <Badge color="ghost" className="font-bold mr-auto">{streams.data.length}</Badge>
                {props.showCascade ? (
                    <Button
                        size="sm"
                        color="ghost"
                        startIcon={<ArrowRightEndOnRectangleIcon className="size-4 stroke-current" />}
                        onClick={handleCascadePullStream}
                    >Cascade Pull
                    </Button>
                ) : null}
                <Button
                    size="sm"
                    color="ghost"
                    endIcon={<Checkbox size="xs" checked={streams.isRefreshing} />}
                    onClick={streams.toggleTimer}
                >Auto Refresh</Button>
                <Button
                    size="sm"
                    color="ghost"
                    endIcon={<ArrowPathIcon className="size-4 stroke-current" />}
                    onClick={streams.updateData}
                >Refresh</Button>
            </div>

            <Table className="overflow-x-auto">
                <Table.Head>
                    <span>ID</span>
                    <span>Publisher</span>
                    <span>Subscriber</span>
                    <span>Cascade</span>
                    <span>Creation Time</span>
                    <span>Operation</span>
                </Table.Head>
                <Table.Body>
                    {streams.data.length > 0 ? streams.data.map(i =>
                        <Table.Row>
                            <span>{i.id}</span>
                            <span>{i.publish.sessions.length}</span>
                            <span>{i.subscribe.sessions.length}</span>
                            <span>{i.publish.sessions.filter(t => t.cascade).length + i.subscribe.sessions.filter(t => t.cascade).length}</span>
                            <span>{formatTime(i.createdAt)}</span>
                            <div className="flex gap-1">
                                <Button
                                    size="sm"
                                    color={previewStreams.includes(i.id) ? 'info' : undefined}
                                    onClick={() => handlePreview(i.id)}
                                >Preview</Button>
                                <Button size="sm" onClick={() => handleViewClients(i.id)}>Clients</Button>
                                {props.showCascade
                                    ? <Button size="sm" onClick={() => handleCascadePushStream(i.id)}>Cascade Push</Button>
                                    : null
                                }
                                <Button size="sm" onClick={() => handleOpenPlayerPage(i.id)}>Player</Button>
                                <Button size="sm" onClick={() => handleOpenDebuggerPage(i.id)}>Debugger</Button>
                                {props.renderExtraActions?.(i)}
                                <Button size="sm" color="error" onClick={() => handleDestroyStream(i.id)}>Destroy</Button>
                            </div>
                        </Table.Row>
                    ) : <tr><td colspan={6} className="text-center">N/A</td></tr>}
                </Table.Body>
            </Table>

            <div className="flex gap-2 p-4">
                <Button
                    size="sm"
                    color="primary"
                    startIcon={<PlusIcon className="size-5 stroke-current" />}
                    onClick={handleNewStream}
                >New Stream</Button>
                {webStreams.map(s =>
                    <Button size="sm" onClick={() => handleOpenWebStream(s)}>{s}</Button>
                )}
            </div>

            <ClientsDialog
                ref={refClients}
                id={selectedStreamId}
                sessions={streams.data.find(s => s.id == selectedStreamId)?.subscribe.sessions ?? []}
                onClientKicked={streams.updateData}
            />

            {props.showCascade ? (
                <>
                    <CascadePullDialog ref={refCascadePull} />
                    <CascadePushDialog ref={refCascadePush} />
                </>
            ) : null}

            {previewStreams.map(s =>
                <PreviewDialog
                    key={s}
                    ref={(instance: IPreviewDialog | null) => {
                        if (instance) {
                            refPreviewStreams.current.set(s, instance);
                        } else {
                            refPreviewStreams.current.delete(s);
                        }
                    }}
                    getWhepUrl={props.getWhepUrl}
                    onStop={() => handlePreviewStop(s)}
                />
            )}

            <NewStreamDialog ref={refNewStream} onNewStreamId={handleNewStreamId} onStreamCreated={streams.updateData} />

            {webStreams.map(s =>
                <WebStreamDialog
                    key={s}
                    ref={(instance: IWebStreamDialog | null) => {
                        if (instance) {
                            refWebStreams.current.set(s, instance);
                        } else {
                            refWebStreams.current.delete(s);
                        }
                    }}
                    getWhipUrl={props.getWhipUrl}
                    onStop={() => handleWebStreamStop(s)}
                />
            )}
        </>
    );
}
