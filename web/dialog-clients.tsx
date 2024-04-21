import { useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef } from 'preact/compat'
import { SubscribeSessionInfo, delStream } from './api'
import { formatTime } from './utils'

interface Props {
    id: string
    clients: SubscribeSessionInfo[]
}

export interface IClientsDialog {
    show(): void
}

export const ClientsDialog = forwardRef<IClientsDialog, Props>((props: Props, ref) => {
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: () => {
                refDialog.current?.showModal()
            }
        }
    })

    return (
        <dialog ref={refDialog}>
            <h3>Clients</h3>
            <table>
                <thead>
                    <tr>
                        <th>ID</th>
                        <th>State</th>
                        <th>Creation Time</th>
                        <th>Operation</th>
                    </tr>
                </thead>
                <tbody>
                    {props.clients.map(c =>
                        <tr>
                            <td>{c.id} {c.reforward ? "(reforward)" : ""}</td>
                            <td>{c.connectState}</td>
                            <td>{formatTime(c.createTime)}</td>
                            <td><button onClick={() => delStream(props.id, c.id)}>Kick</button></td>
                        </tr>
                    )}
                </tbody>
            </table>
            <br />
            <button onClick={() => refDialog.current?.close()}>Close</button>
        </dialog>
    )
})
