import { useState, useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef, TargetedEvent } from 'preact/compat';

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

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setReforwardURL(e.currentTarget.value)
    }

    return (
        <dialog ref={refDialog}>
            <form method="dialog">
                <h3>Reforward</h3>
                <p>
                    <label htmlFor="reforward-url">Target URL:</label>
                    <br />
                    <input
                        type="text"
                        value={reforwardURL}
                        id="reforward-url"
                        className="min-w-sm"
                        onChange={handleURLInputChange}
                    />
                </p>
                <div>
                    <button value="">Cancel</button>
                    <button value={reforwardURL}>Confirm</button>
                </div>
            </form>
        </dialog>
    )
})
