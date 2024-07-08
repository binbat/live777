import { useState, useRef, useEffect } from 'preact/hooks'

import { getStreams, deleteStream } from '../api'
import { formatTime, nextSeqId } from '../utils'
import { useRefreshTimer } from '../hooks/use-refresh-timer'
import { StyledCheckbox } from './styled-checkbox'
import { IClientsDialog, ClientsDialog } from './dialog-clients'
import { ICascadeDialog, CascadePullDialog, CascadePushDialog } from './dialog-cascade'
import { IPreviewDialog, PreviewDialog } from './dialog-preview'
import { IWebStreamDialog, WebStreamDialog } from './dialog-web-stream'
import { INewStreamDialog, NewStreamDialog } from './dialog-new-stream'

export function StreamsTable() {
    const streams = useRefreshTimer([], getStreams)
    const [selectedStreamId, setSelectedStreamId] = useState('')
    const refCascadePull = useRef<ICascadeDialog>(null)
    const refCascadePush = useRef<ICascadeDialog>(null)
    const refClients = useRef<IClientsDialog>(null)
    const refNewStream = useRef<INewStreamDialog>(null)
    const [webStreams, setWebStreams] = useState<string[]>([])
    const [newStreamId, setNewStreamId] = useState('')
    const refWebStreams = useRef<Map<string, IWebStreamDialog>>(new Map())
    const [previewStreams, setPreviewStreams] = useState<string[]>([])
    const [previewStreamId, setPreviewStreamId] = useState('')
    const refPreviewStreams = useRef<Map<string, IPreviewDialog>>(new Map())

    const handleViewClients = (id: string) => {
        setSelectedStreamId(id)
        refClients.current?.show()
    }

    const handleCascadePullStream = () => {
        const newStreamId = nextSeqId('pull-', streams.data.map(s => s.id))
        refCascadePull.current?.show(newStreamId)
    }

    const handleCascadePushStream = (id: string) => {
        refCascadePush.current?.show(id)
    }

    const handlePreview = (id: string) => {
        if (previewStreams.includes(id)) {
            refPreviewStreams.current.get(id)?.show(id)
        } else {
            setPreviewStreams([...previewStreams, id])
            setPreviewStreamId(id)
        }
    }

    useEffect(() => {
        refPreviewStreams.current.get(previewStreamId)?.show(previewStreamId)
    }, [previewStreamId])

    const handlePreviewStop = (id: string) => {
        setPreviewStreamId('')
        setPreviewStreams(previewStreams.filter(s => s !== id))
    }

    const handleNewStream = () => {
        const newStreamId = nextSeqId('web-', webStreams.concat(streams.data.map(s => s.id)))
        refNewStream.current?.show(newStreamId)
    }

    const handleNewStreamId = (id: string) => {
        setWebStreams([...webStreams, id])
        setNewStreamId(id)
    }

    useEffect(() => {
        refWebStreams.current.get(newStreamId)?.show(newStreamId)
    }, [newStreamId])

    const handleOpenWebStream = (id: string) => {
        refWebStreams.current.get(id)?.show(id)
    }

    const handleWebStreamStop = (id: string) => {
        setNewStreamId('')
        setWebStreams(webStreams.filter(s => s !== id))
    }

    const handleOpenPlayerPage = (id: string) => {
        const params = new URLSearchParams()
        params.set('id', id)
        params.set('autoplay', '')
        params.set('muted', '')
        params.set('reconnect', '')
        const url = new URL(`/tools/player.html?${params.toString()}`, location.origin)
        window.open(url)
    }

    const handleOpenDebuggerPage = (id: string) => {
        const params = new URLSearchParams()
        params.set('id', id)
        const url = new URL(`/tools/debugger.html?${params.toString()}`, location.origin)
        window.open(url)
    }

    return (
        <>
            <ClientsDialog ref={refClients} id={selectedStreamId} sessions={streams.data.find(s => s.id == selectedStreamId)?.subscribe.sessions ?? []} />

            <CascadePullDialog ref={refCascadePull} />
            <CascadePushDialog ref={refCascadePush} />

            {previewStreams.map(s =>
                <PreviewDialog
                    key={s}
                    ref={(instance: IPreviewDialog | null) => {
                        if (instance) {
                            refPreviewStreams.current.set(s, instance)
                        } else {
                            refPreviewStreams.current.delete(s)
                        }
                    }}
                    onStop={() => { handlePreviewStop(s) }}
                />
            )}

            <NewStreamDialog ref={refNewStream} onNewStreamId={handleNewStreamId} />

            {webStreams.map(s =>
                <WebStreamDialog
                    key={s}
                    ref={(instance: IWebStreamDialog | null) => {
                        if (instance) {
                            refWebStreams.current.set(s, instance)
                        } else {
                            refWebStreams.current.delete(s)
                        }
                    }}
                    onStop={() => { handleWebStreamStop(s) }}
                />
            )}

            <fieldset>
                <legend class="inline-flex items-center gap-x-4">
                    <span>Streams (total: {streams.data.length})</span>
                    <button onClick={() => streams.updateData()}>Refresh</button>
                    <StyledCheckbox label="Auto Refresh" checked={streams.isRefreshing} onClick={streams.toggleTimer}></StyledCheckbox>
                </legend>
                <table>
                    <thead>
                        <tr>
                            <th class="min-w-12">ID</th>
                            <th>Publisher</th>
                            <th>Subscriber</th>
                            <th>Cascade</th>
                            <th class="min-w-72">Creation Time</th>
                            <th class="min-w-72">Operation</th>
                        </tr>
                    </thead>
                    <tbody>
                        {streams.data.map(i =>
                            <tr>
                                <td class="text-center">{i.id}</td>
                                <td class="text-center">{i.publish.sessions.length}</td>
                                <td class="text-center">{i.subscribe.sessions.length}</td>
                                <td class="text-center">{
                                    i.publish.sessions.filter((t: any) => t.cascade).length +
                                    i.subscribe.sessions.filter((t: any) => t.cascade).length
                                }</td>
                                <td class="text-center">{formatTime(i.createdAt)}</td>
                                <td>
                                    <button onClick={() => handlePreview(i.id)} class={previewStreams.includes(i.id) ? 'text-blue-500' : undefined} >Preview</button>
                                    <button onClick={() => handleViewClients(i.id)}>Clients</button>
                                    <button onClick={() => handleCascadePushStream(i.id)}>Cascade Push</button>
                                    <button onClick={() => handleOpenPlayerPage(i.id)}>Player</button>
                                    <button onClick={() => handleOpenDebuggerPage(i.id)}>Debugger</button>
                                    <button onClick={() => deleteStream(i.id)} class="text-red-500">Destroy</button>
                                </td>
                            </tr>
                        )}
                    </tbody>
                </table>
                <div>
                    <button onClick={handleCascadePullStream}>Cascade Pull</button>
                    <button onClick={handleNewStream}>New Stream</button>
                    {webStreams.map(s =>
                        <button onClick={() => { handleOpenWebStream(s) }}>{s}</button>
                    )}
                </div>
            </fieldset>
        </>
    )
}
