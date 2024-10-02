import { useState, useRef, useImperativeHandle } from 'preact/hooks';
import { TargetedEvent, forwardRef } from 'preact/compat';

import { createStream } from '../api';

interface Props {
    onNewStreamId(id: string): void
}

export interface INewStreamDialog {
    show(initialId: string): void
}

export const NewStreamDialog = forwardRef<INewStreamDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState('');
    const refDialog = useRef<HTMLDialogElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: (initialId: string) => {
                setStreamId(initialId);
                refDialog.current?.showModal();
            }
        };
    });

    const onStreamIdInput = (e: TargetedEvent<HTMLInputElement>) => {
        setStreamId(e.currentTarget.value);
    };

    const onConfirmNewStreamId = (_e: Event) => {
        props.onNewStreamId(streamId);
        createStream(streamId);
    };

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
    );
});
