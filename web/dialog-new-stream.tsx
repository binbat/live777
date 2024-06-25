import { useState, useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef } from 'preact/compat';

interface Props {
    onNewStreamId(id: string): void
}

export interface INewStreamDialog {
    show(initialId: string): void
}

export const NewStreamDialog = forwardRef<INewStreamDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (initialId: string) => {
                setStreamId(initialId)
                refDialog.current?.showModal()
            }
        }
    })

    const onStreamIdInput = (e: Event) => {
        setStreamId((e.target as HTMLInputElement).value)
    }

    const onConfirmNewStreamId = (_e: Event) => {
        props.onNewStreamId(streamId)
        refDialog.current?.close()
    }

    return (
        <dialog ref={refDialog}>
            <h3>New Stream</h3>
            <p>
                <label>Stream ID:
                    <input type="text" value={streamId} onChange={onStreamIdInput} />
                </label>
            </p>
            <form method="dialog">
                <button>Cancel</button>
                <button onClick={onConfirmNewStreamId}>Confirm</button>
            </form>
        </dialog>
    )
})
