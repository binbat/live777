import { useState, useRef, useImperativeHandle } from 'preact/hooks'
import { forwardRef, TargetedEvent } from 'preact/compat';

import { cascade } from '../api'

export interface ICascadeDialog {
    show(streamId: string): void
}

export const CascadePullDialog = forwardRef<ICascadeDialog>((_props, ref) => {
    const [streamId, setStreamId] = useState('')
    const [cascadeURL, setCascadeURL] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setStreamId(streamId)
                setCascadeURL(`${location.origin}/whep/`)
                refDialog.current?.showModal()
            }
        }
    })

    const handleStreamIdInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setStreamId(e.currentTarget.value)
    }

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setCascadeURL(e.currentTarget.value)
    }

    const onConfirmCascadeURL = (_e: Event) => {
        if (cascadeURL !== '') {
            cascade(streamId, {
                sourceUrl: cascadeURL,
            })
        }
    }

    return (
        <dialog ref={refDialog}>
            <h3>Cascade Pull</h3>
            <p>
                <label>Stream ID:
                    <br />
                    <input className="min-w-sm" value={streamId} onChange={handleStreamIdInputChange} />
                </label>
            </p>
            <p>
                <label>Source URL:
                    <br />
                    <input className="min-w-sm" value={cascadeURL} onChange={handleURLInputChange} />
                </label>
            </p>
            <form method="dialog">
                <button>Cancel</button>
                <button onClick={onConfirmCascadeURL}>Confirm</button>
            </form>
        </dialog>
    )
})

export const CascadePushDialog = forwardRef<ICascadeDialog>((_props, ref) => {
    const [streamId, setStreamId] = useState('')
    const [cascadeURL, setCascadeURL] = useState('')
    const refDialog = useRef<HTMLDialogElement>(null)

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setStreamId(streamId)
                setCascadeURL(`${location.origin}/whip/push`)
                refDialog.current?.showModal()
            }
        }
    })

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setCascadeURL(e.currentTarget.value)
    }

    const onConfirmCascadeURL = (_e: Event) => {
        if (cascadeURL !== '') {
            cascade(streamId, {
                targetUrl: cascadeURL,
            })
        }
    }

    return (
        <dialog ref={refDialog}>
            <h3>Cascade Push ({streamId})</h3>
            <p>
                <label>Target URL:
                    <br />
                    <input className="min-w-sm" value={cascadeURL} onChange={handleURLInputChange} />
                </label>
            </p>
            <form method="dialog">
                <button>Cancel</button>
                <button onClick={onConfirmCascadeURL}>Confirm</button>
            </form>
        </dialog>
    )
})
