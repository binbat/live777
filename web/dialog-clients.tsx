import { useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef } from 'preact/compat'
import { Session, delStream } from './api'
import { formatTime } from './utils'

interface Props {
    id: string
    sessions: Session[]
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
            <h3>Clients of {props.id}</h3>
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
                    {props.sessions.map(c =>
                        <tr>
                            <td>{c.id} {c.reforward ? "(reforward)" : ""}</td>
                            <td>{c.state}</td>
                            <td>{formatTime(c.createdAt)}</td>
                            <td><button onClick={() => delStream(props.id, c.id)}>Kick</button></td>
                        </tr>
                    )}
                </tbody>
            </table>
            <form method="dialog">
                <button>Close</button>
            </form>
        </dialog>
    )
})
