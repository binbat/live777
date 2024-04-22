import { useState, useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef } from 'preact/compat';

interface Props {
    onNewResourceId(id: string): void
}

export interface INewStreamDialog {
    show(initialId: string): void
}

export const NewStreamDialog = forwardRef<INewStreamDialog, Props>((props, ref) => {
    const [resoruceId, setResoruceId] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (initialId: string) => {
                setResoruceId(initialId)
                refDialog.current?.showModal()
            }
        }
    })

    const onResourceIdInput = (e: Event) => {
        setResoruceId((e.target as HTMLInputElement).value)
    }

    const onConfirmNewResourceId = (_e: Event) => {
        props.onNewResourceId(resoruceId)
        refDialog.current?.close()
    }

    return (
        <dialog ref={refDialog}>
            <h3>New Stream</h3>
            <p>
                <label>Resource ID:
                    <input type="text" value={resoruceId} onChange={onResourceIdInput} />
                </label>
            </p>
            <form method="dialog">
                <button>Cancel</button>
                <button onClick={onConfirmNewResourceId}>Confirm</button>
            </form>
        </dialog>
    )
})
