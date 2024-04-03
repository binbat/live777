import { useState, useRef } from 'preact/hooks'
import Logo from '/logo.svg'
import './app.css'

// - stream
// - client
async function delStream(streamId: string, clientId: string) {
    return fetch(`/resource/${streamId}/${clientId}`, {
        method: "DELETE",
    })
}

async function allStream(): Promise<any[]> {
    return (await fetch("/admin/infos")).json()
}

async function reforward(streamId: string, url: string): Promise<void> {
    fetch(`/admin/re-forward/${streamId}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
        },
        body: JSON.stringify({
            whipUrl: url,
        }),
    })
}

export function App() {
    const [items, setItems] = useState<any[]>([])
    const refTimer = useRef<null | ReturnType<typeof setInterval>>(null)
    const refDialog = useRef<HTMLDialogElement>(null)
    const refConfirm = useRef<HTMLButtonElement>(null)
    const refInput = useRef<HTMLInputElement>(null)

    const triggerTimer = () => {
        if (refTimer.current) {
            clearInterval(refTimer.current)
            refTimer.current = null
        } else {
            refTimer.current = setInterval(async () => setItems(await allStream()), 3000)
        }
    }

    const triggerForward = (streamId: string) => {
        refDialog.current?.showModal()

        if (refInput.current) refInput.current.value = ""

        if (refDialog.current) refDialog.current.onclose = () => {
            //whipUrl: "http://localhost:7777/whip/888",
            const target = refDialog.current?.returnValue
            console.log(target)
            if (target) reforward(streamId, target)
        }
    }

    return (
        <>
            <div>
                <a href="https://live777.binbat.com" target="_blank">
                    <img src={Logo} class="logo" alt="Live777 logo" />
                </a>
            </div>

            <label class="inline-flex items-center cursor-pointer">
                <input type="checkbox" value="" class="sr-only peer" checked={!!refTimer.current} onClick={triggerTimer} />
                <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                <span class="ms-3 text-sm font-medium dark:text-gray-300">Auto Refresh</span>
            </label>

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

            <table>
                <thead>
                    <tr>
                        <th>Id</th>
                        <th>Publisher</th>
                        <th>Subscriber</th>
                        <th>Reforward</th>
                        <th>Create Time</th>
                        <th>Operate</th>
                    </tr>
                </thead>
                <tbody>
                    {items.map(i => <tr>
                        <td>{i.id}</td>
                        <td>{i.publishLeaveTime === 0 ? "Ok" : "No"}</td>
                        <td>{i.subscribeSessionInfos.length}</td>
                        <th>{i.subscribeSessionInfos.filter((t: any) => t.reForward).length}</th>
                        <td>{i.createTime}</td>
                        <td>
                            <button onClick={ () => delStream(i.id, i.publishSessionInfo.id) }>Destroy</button>
                            <button onClick={ () => triggerForward(i.id)}>Reforward</button>
                        </td>
                    </tr>)}
                </tbody>
                <tfoot>
                    <tr>
                        <th colspan={4}>Total</th>
                        <td>{items.length}</td>
                    </tr>
                </tfoot>
            </table>
        </>
    )
}
