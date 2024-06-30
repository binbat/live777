import { useState, useRef, useEffect } from 'preact/hooks'

import { Stream, allStream, delStream } from '../api'
import { formatTime } from '../utils'

import { IClientsDialog, ClientsDialog } from './dialog-clients'
import { ICascadeDialog, CascadePullDialog, CascadePushDialog } from './dialog-cascade'
import { IPreviewDialog, PreviewDialog } from './dialog-preview'
import { IWebStreamDialog, WebStreamDialog } from './dialog-web-stream'
import { INewStreamDialog, NewStreamDialog } from './dialog-new-stream'

export function StreamsTable() {
    const [streams, setStreams] = useState<Stream[]>([])
    const [selectedStreamId, setSelectedStreamId] = useState('')
    const [refreshTimer, setRefershTimer] = useState(-1)
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

    const updateAllStreams = async () => {
        setStreams(await allStream())
    }

    // fetch all streams on component mount
    useEffect(() => { updateAllStreams() }, [])

    const toggleTimer = () => {
        if (refreshTimer > 0) {
            clearInterval(refreshTimer)
            setRefershTimer(-1)
        } else {
            updateAllStreams()
            setRefershTimer(window.setInterval(updateAllStreams, 3000))
        }
    }

    const handleViewClients = (id: string) => {
        setSelectedStreamId(id)
        refClients.current?.show()
    }

    const handleCascadePullStream = (id: string) => {
        refCascadePull.current?.show(id)
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
        const prefix = 'web-'
        const existingIds = webStreams.concat(streams.filter(s => s.id.startsWith(prefix)).map(s => s.id))
        let i = 0
        let newStreamId = `${prefix}${i}`
        while (existingIds.includes(newStreamId)) {
            i++
            newStreamId = `${prefix}${i}`
        }
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

    return (
        <>
            <ClientsDialog ref={refClients} id={selectedStreamId} sessions={streams.find(s => s.id == selectedStreamId)?.subscribe.sessions ?? []} />

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
                <legend class="inline-flex items-center">
                    <span>Streams (total: {streams.length})</span>
                    <label class="ml-10 inline-flex items-center cursor-pointer">
                        <input type="checkbox" class="sr-only peer" checked={refreshTimer > 0} onClick={toggleTimer} />
                        <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                        <span class="ml-2">Auto Refresh</span>
                    </label>
                </legend>
                <legend>
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
                        {streams.map(i =>
                            <tr>
                                <td class="text-center">{i.id}</td>
                                <td class="text-center">{i.publish.sessions.length}</td>
                                <td class="text-center">{i.subscribe.sessions.length}</td>
                                <td class="text-center">{i.subscribe.sessions.filter((t: any) => t.reforward).length}</td>
                                <td class="text-center">{formatTime(i.createdAt)}</td>
                                <td>
                                    <button onClick={() => handlePreview(i.id)} class={previewStreams.includes(i.id) ? 'text-blue-500' : undefined} >Preview</button>
                                    <button onClick={() => handleViewClients(i.id)}>Clients</button>
                                    <button onClick={() => handleCascadePullStream(i.id)}>Cascade Pull</button>
                                    <button onClick={() => handleCascadePushStream(i.id)}>Cascade Push</button>
                                    <button onClick={() => delStream(i.id, i.publish.sessions[0].id)} class="text-red-500">Destroy</button>
                                </td>
                            </tr>
                        )}
                    </tbody>
                </table>
                <div>
                    <button onClick={handleNewStream}>New Stream</button>
                    {webStreams.map(s =>
                        <button onClick={() => { handleOpenWebStream(s) }}>{s}</button>
                    )}
                </div>
            </fieldset>
        </>
    )
}
