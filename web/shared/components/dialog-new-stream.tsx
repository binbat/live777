import { useState, useRef, useImperativeHandle } from "preact/hooks";
import { TargetedEvent, forwardRef } from "preact/compat";
import { Button, Input, Modal } from "react-daisyui";

import { createStream } from "../api";

interface Props {
    onNewStreamId(id: string): void;
    onStreamCreated(): void;
}

export interface INewStreamDialog {
    show(initialId: string): void;
}

export const NewStreamDialog = forwardRef<INewStreamDialog, Props>((props, ref) => {
    const [streamId, setStreamId] = useState("");
    const refDialog = useRef<HTMLDialogElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: (initialId: string) => {
                setStreamId(initialId);
                refDialog.current?.showModal();
            },
        };
    });

    const onStreamIdInput = (e: TargetedEvent<HTMLInputElement>) => {
        setStreamId(e.currentTarget.value);
    };

    const onConfirmNewStreamId = async (_e: Event) => {
        try {
            props.onNewStreamId(streamId);
            await createStream(streamId);
            props.onStreamCreated();
        } catch (error) {
            console.error("Failed to create stream:", error);
        }
    };
    return (
        <Modal ref={refDialog} className="max-w-md">
            <Modal.Header className="mb-2">
                <h3 className="font-bold">New Stream</h3>
            </Modal.Header>
            <Modal.Body>
                <div className="form-control">
                    <label className="label px-0">Stream ID:</label>
                    <Input borderOffset value={streamId} onChange={onStreamIdInput} />
                </div>
            </Modal.Body>
            <Modal.Actions>
                <form method="dialog" className="flex gap-2">
                    <Button onClick={onConfirmNewStreamId}>Confirm</Button>
                    <Button>Cancel</Button>
                </form>
            </Modal.Actions>
        </Modal>
    );
});
