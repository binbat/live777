import { useState, useRef, useEffect } from 'preact/hooks'
import Logo from '/logo.svg'
import './app.css'
import { Dialog } from './dialog'
import {
    allStream,
    delStream,
    reforward,
} from './api'

const formatTime = (timestamp: number) => new Date(timestamp).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23'
});

export function App() {
    const [streamId, setStreamId] = useState<string>("")
    const [items, setItems] = useState<any[]>([])
    const [pubItems, setPubItems] = useState<any[]>([])
    const refTimer = useRef<null | ReturnType<typeof setInterval>>(null)
    const refDialog = useRef<HTMLDialogElement>(null)
    const refConfirm = useRef<HTMLButtonElement>(null)
    const refInput = useRef<HTMLInputElement>(null)

    const updateAllStreams = async () => {
        setItems(await allStream())
    }

    // fetch all streams on component mount
    useEffect(() => { updateAllStreams() }, [])

    const triggerTimer = () => {
        if (refTimer.current) {
            clearInterval(refTimer.current)
            refTimer.current = null
        } else {
            updateAllStreams()
            refTimer.current = setInterval(updateAllStreams, 3000)
        }
    }

    const triggerForward = (streamId: string) => {
        refDialog.current?.showModal()

        if (refInput.current) refInput.current.value = ""

        if (refDialog.current) refDialog.current.onclose = () => {
            //targetUrl: "http://localhost:7777/whip/888",
            const target = refDialog.current?.returnValue
            console.log(target)
            if (target) reforward(streamId, target)
        }
    }

    return (
        <>
            <div class="flex flex-justify-center">
                <a href="https://live777.binbat.com" target="_blank">
                    <img src={Logo} class="logo" alt="Live777 logo" />
                </a>
            </div>

            <Dialog streamId={streamId} items={pubItems} />

            <dialog ref={refDialog}>
                <form method="dialog">
                    <p>
                        <label
                        >Target Url:
                            <input ref={refInput} type="text" onChange={e => {
                                if (refConfirm.current && e.target) {
                                    //@ts-ignore
                                    refConfirm.current.value = e.target.value
                                }
                            }} />
                        </label>
                    </p>
                    <div>
                        <button value="">Cancel</button>
                        <button ref={refConfirm} value="">Confirm</button>
                    </div>
                </form>
            </dialog>

            <fieldset>
                <legend class="inline-flex items-center">
                    <span>Streams (total: {items.length})</span>
                    <label class="ml-10 inline-flex items-center cursor-pointer">
                        <input type="checkbox" value="" class="sr-only peer" checked={!!refTimer.current} onClick={triggerTimer} />
                        <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                        <span class="ml-2">Auto Refresh</span>
                    </label>
                </legend>
                <legend>
                </legend>
                <table>
                    <thead>
                        <tr>
                            <th class="mw-50 text-center">ID</th>
                            <th>Publisher</th>
                            <th>Subscriber</th>
                            <th>Reforward</th>
                            <th class="mw-300">Creation Time</th>
                            <th class="mw-300">Operation</th>
                        </tr>
                    </thead>
                    <tbody>
                        {items.map(i =>
                            <tr>
                                <td class="text-center">{i.id}</td>
                                <td class="text-center">{i.publishLeaveTime === 0 ? "Ok" : "No"}</td>
                                <td class="text-center">{i.subscribeSessionInfos.length}</td>
                                <td class="text-center">{i.subscribeSessionInfos.filter((t: any) => t.reforward).length}</td>
                                <td class="text-center">{formatTime(i.createTime)}</td>
                                <td>
                                    <button onClick={() => delStream(i.id, i.publishSessionInfo.id)}>Destroy</button>
                                    <button onClick={() => {
                                        setStreamId(i.id)
                                        setPubItems(i.subscribeSessionInfos)
                                    }}>Kick</button>
                                    <button onClick={() => triggerForward(i.id)}>Reforward</button>
                                </td>
                            </tr>
                        )}
                    </tbody>
                </table>
            </fieldset>
        </>
    )
}
