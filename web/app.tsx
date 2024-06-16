import { useState, useRef, useEffect } from 'preact/hooks'
import Logo from '/logo.svg'
import './app.css'
import { Stream, allStream, delStream } from './api'
import { formatTime } from './utils'
import { IClientsDialog, ClientsDialog } from './dialog-clients'
import { IReforwardDialog, ReforwardDialog } from './dialog-reforward'
import { IPreviewDialog, PreviewDialog } from './dialog-preview'
import { IWebStreamDialog, WebStreamDialog } from './dialog-web-stream'
import { INewStreamDialog, NewStreamDialog } from './dialog-new-stream'

export function App() {
    const [streams, setStreams] = useState<Stream[]>([])
    const [selectedStreamId, setSelectedStreamId] = useState('')
    const [refreshTimer, setRefershTimer] = useState(-1)
    const refReforward = useRef<IReforwardDialog>(null)
    const refClients = useRef<IClientsDialog>(null)
    const refPreview = useRef<IPreviewDialog>(null)
    const refNewStream = useRef<INewStreamDialog>(null)
    const [webStreams, setWebStreams] = useState<string[]>([])
    const [newResourceId, setNewResourceId] = useState('')
    const refWebStreams = useRef<Map<string, IWebStreamDialog>>(new Map())

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

    const handleReforwardStream = (id: string) => {
        refReforward.current?.show(id)
    }

    const handlePreview = (id: string) => {
        refPreview.current?.show(id)
    }

    const handleNewStream = () => {
        const prefix = 'web-'
        const existingIds = webStreams.concat(streams.filter(s => s.id.startsWith(prefix)).map(s => s.id))
        let i = 0
        let newResourceId = `${prefix}${i}`
        while (existingIds.includes(newResourceId)) {
            i++
            newResourceId = `${prefix}${i}`
        }
        refNewStream.current?.show(newResourceId)
    }

    const handleNewResourceId = (id: string) => {
        setWebStreams([...webStreams, id])
        setNewResourceId(id)
    }

    useEffect(() => {
        refWebStreams.current.get(newResourceId)?.show(newResourceId)
    }, [newResourceId])

    const handleOpenWebStream = (id: string) => {
        refWebStreams.current.get(id)?.show(id)
    }

    const handleWebStreamStop = (id: string) => {
        setWebStreams(webStreams.filter(s => s !== id))
    }

    return (
        <>
            <div class="flex flex-justify-center">
                <a href="https://live777.binbat.com" target="_blank">
                    <img src={Logo} class="logo" alt="Live777 logo" />
                </a>
            </div>

            <ClientsDialog ref={refClients} id={selectedStreamId} sessions={streams.find(s => s.id == selectedStreamId)?.subscribe.sessions ?? []} />

            <ReforwardDialog ref={refReforward} />

            <PreviewDialog ref={refPreview} />

            <NewStreamDialog ref={refNewStream} onNewResourceId={handleNewResourceId} />

            {webStreams.map(s =>
                <WebStreamDialog
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
                        <input type="checkbox" value="" class="sr-only peer" checked={refreshTimer > 0} onClick={toggleTimer} />
                        <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                        <span class="ml-2">Auto Refresh</span>
                    </label>
                </legend>
                <legend>
                </legend>
                <table>
                    <thead>
                        <tr>
                            <th class="mw-50">ID</th>
                            <th>Publisher</th>
                            <th>Subscriber</th>
                            <th>Reforward</th>
                            <th class="mw-300">Creation Time</th>
                            <th class="mw-300">Operation</th>
                        </tr>
                    </thead>
                    <tbody>
                        {streams.map(i =>
                            <tr>
                                <td class="text-center">{i.id}</td>
                                <td class="text-center">{i.publish.leaveAt === 0 ? "Ok" : "No"}</td>
                                <td class="text-center">{i.subscribe.sessions.length}</td>
                                <td class="text-center">{i.subscribe.sessions.filter((t: any) => t.reforward).length}</td>
                                <td class="text-center">{formatTime(i.createdAt)}</td>
                                <td>
                                    <button onClick={() => handlePreview(i.id)}>Preview</button>
                                    <button onClick={() => handleViewClients(i.id)}>Clients</button>
                                    <button onClick={() => handleReforwardStream(i.id)}>Reforward</button>
                                    <button style={{ color: 'red' }} onClick={() => delStream(i.id, i.publish.sessions[0].id)}>Destroy</button>
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
