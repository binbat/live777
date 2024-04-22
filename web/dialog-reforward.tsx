import { useState, useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef } from 'preact/compat';
import { reforward } from './api'

export interface IReforwardDialog {
    show(streamId: string): void
}

export const ReforwardDialog = forwardRef<IReforwardDialog>((_props, ref) => {
    const [reforwardURL, setReforwardURL] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setReforwardURL('')
                if (refDialog.current) {
                    refDialog.current.onclose = () => {
                        // example: http://localhost:7777/whip/888
                        const target = refDialog.current?.returnValue ?? ''
                        if (target !== '') {
                            reforward(streamId, target)
                        }
                    }
                    refDialog.current.showModal()
                }
            }
        }
    })

    return (
        <dialog ref={refDialog}>
            <form method="dialog">
                <h3>Reforward</h3>
                <p>
                    <label>Target URL:
                        <input type="text" value={reforwardURL} onChange={e => setReforwardURL((e.target as HTMLInputElement)?.value)} />
                    </label>
                </p>
                <div>
                    <button value="">Cancel</button>
                    <button value={reforwardURL}>Confirm</button>
                </div>
            </form>
        </dialog>
    )
})
