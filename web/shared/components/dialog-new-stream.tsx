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

    function getCurrentNode(): string | null {
        const urlParams = new URLSearchParams(window.location.search);
        return urlParams.get("nodes");
    }

    async function handleCreateStream(streamId: string) {
        const currentNode = getCurrentNode();
        try {
            await createStream(streamId, currentNode);
            console.log(`Stream ${streamId} created successfully`);
            return true;
        } catch (error: unknown) {
            if (
                error instanceof Object &&
                "response" in error &&
                typeof (error as { response: { status: number } }).response.status === "number" &&
                (error as { response: { status: number } }).response.status === 409
            ) {
                window.alert("Resource already exists, please use a different streamId");
                return false;
            }
            console.error("Failed to create stream:", error);
            window.alert("Failed to create stream, please try again later");
            return false;
        }
    }

    const onConfirmNewStreamId = async (_e: Event) => {
        if (!streamId.trim()) {
            window.alert("Please enter a valid Stream ID");
            return;
        }

        props.onNewStreamId(streamId);
        const success = await handleCreateStream(streamId);
        if (success) {
            props.onStreamCreated();
            refDialog.current?.close();
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
                    <Button onClick={() => refDialog.current?.close()}>Cancel</Button>
                </form>
            </Modal.Actions>
        </Modal>
    );
});
