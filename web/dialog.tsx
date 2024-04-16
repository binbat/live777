import { useEffect, useRef } from 'preact/hooks'
import {
    delStream,
} from './api'

export function Dialog(props: { streamId: string, items: any[] }) {
    const refDialog = useRef<HTMLDialogElement>(null)

    useEffect(() => {
        if (props.items.length > 0) refDialog.current?.showModal()
    }, [props.items])

    return (
        <dialog ref={refDialog}>
            <div class="flex flex-col">
                {props.items.map(i => <a onClick={() => delStream(props.streamId, i.id)}>{i.id} {i.reforward ? "reforward" : ""}</a>)}
            </div>
            <br/><button onClick={() => refDialog.current?.close()}>Close</button>
        </dialog>
    )
}
