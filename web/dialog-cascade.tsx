import { useState, useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef, TargetedEvent } from 'preact/compat';

import { cascade } from './api'

export interface ICascadeDialog {
    show(streamId: string): void
}

export const CascadePullDialog = forwardRef<ICascadeDialog>((_props, ref) => {
    const [cascadeURL, setCascadeURL] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setCascadeURL("pull")
                if (refDialog.current) {
                    refDialog.current.onclose = () => {
                        const target = refDialog.current?.returnValue ?? ''
                        if (target !== '') {
                            cascade(target, {
                                src: location.href + "whep/" + streamId,
                            })
                        }
                    }
                    refDialog.current.showModal()
                }
            }
        }
    })

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setCascadeURL(e.currentTarget.value)
    }

    return (
        <dialog ref={refDialog}>
            <form method="dialog">
                <h3>Cascade</h3>
                <p>
                    <label htmlFor="cascade-url">Stream Id:</label>
                    <br />
                    <input
                        type="text"
                        value={cascadeURL}
                        id="cascade-url"
                        className="min-w-sm"
                        onChange={handleURLInputChange}
                    />
                </p>
                <div>
                    <button value="">Cancel</button>
                    <button value={cascadeURL}>Confirm</button>
                </div>
            </form>
        </dialog>
    )
})

export const CascadePushDialog = forwardRef<ICascadeDialog>((_props, ref) => {
    const [cascadeURL, setCascadeURL] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setCascadeURL(location.href + "whip/push")
                if (refDialog.current) {
                    refDialog.current.onclose = () => {
                        const target = refDialog.current?.returnValue ?? ''
                        if (target !== '') {
                            cascade(streamId, {
                                dst: target,
                            })
                        }
                    }
                    refDialog.current.showModal()
                }
            }
        }
    })

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setCascadeURL(e.currentTarget.value)
    }

    return (
        <dialog ref={refDialog}>
            <form method="dialog">
                <h3>Cascade</h3>
                <p>
                    <label htmlFor="cascade-url">Target URL:</label>
                    <br />
                    <input
                        type="text"
                        value={cascadeURL}
                        id="cascade-url"
                        className="min-w-sm"
                        onChange={handleURLInputChange}
                    />
                </p>
                <div>
                    <button value="">Cancel</button>
                    <button value={cascadeURL}>Confirm</button>
                </div>
            </form>
        </dialog>
    )
})
