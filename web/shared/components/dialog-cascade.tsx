import { useState, useRef, useImperativeHandle } from 'preact/hooks';
import { forwardRef, TargetedEvent } from 'preact/compat';
import { Button, Input, Modal } from 'react-daisyui';

import { cascade } from '../api';

export interface ICascadeDialog {
    show(streamId: string): void
}

export const CascadePullDialog = forwardRef<ICascadeDialog>((_props, ref) => {
    const [streamId, setStreamId] = useState('');
    const [cascadeURL, setCascadeURL] = useState('');
    const refDialog = useRef<HTMLDialogElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setStreamId(streamId);
                setCascadeURL(`${location.origin}/whep/`);
                refDialog.current?.showModal();
            }
        };
    });

    const handleStreamIdInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setStreamId(e.currentTarget.value);
    };

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setCascadeURL(e.currentTarget.value);
    };

    const onConfirmCascadeURL = (_e: Event) => {
        if (cascadeURL !== '') {
            cascade(streamId, {
                sourceUrl: cascadeURL,
            });
        }
    };

    return (
        <Modal ref={refDialog} className="max-w-md">
            <Modal.Header className="mb-2">
                <h3 className="font-bold">Cascade Pull</h3>
            </Modal.Header>
            <Modal.Body>
                <div className="form-control">
                    <label className="label px-0">Stream ID:</label>
                    <Input borderOffset value={streamId} onChange={handleStreamIdInputChange} />
                </div>
                <div className="form-control">
                    <label className="label px-0">Source URL:</label>
                    <Input borderOffset value={cascadeURL} onChange={handleURLInputChange} />
                </div>
            </Modal.Body>
            <Modal.Actions>
                <form method="dialog" className="flex gap-2">
                    <Button onClick={onConfirmCascadeURL}>Confirm</Button>
                    <Button>Cancel</Button>
                </form>
            </Modal.Actions>
        </Modal>
    );
});

export const CascadePushDialog = forwardRef<ICascadeDialog>((_props, ref) => {
    const [streamId, setStreamId] = useState('');
    const [cascadeURL, setCascadeURL] = useState('');
    const refDialog = useRef<HTMLDialogElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: (streamId: string) => {
                setStreamId(streamId);
                setCascadeURL(`${location.origin}/whip/push`);
                refDialog.current?.showModal();
            }
        };
    });

    const handleURLInputChange = (e: TargetedEvent<HTMLInputElement>) => {
        setCascadeURL(e.currentTarget.value);
    };

    const onConfirmCascadeURL = (_e: Event) => {
        if (cascadeURL !== '') {
            cascade(streamId, {
                targetUrl: cascadeURL,
            });
        }
    };

    return (
        <Modal ref={refDialog} className="max-w-md">
            <Modal.Header className="mb-2">
                <h3 className="font-bold">Cascade Push ({streamId})</h3>
            </Modal.Header>
            <Modal.Body>
                <div className="form-control">
                    <label className="label px-0">Target URL:</label>
                    <Input borderOffset value={cascadeURL} onChange={handleURLInputChange} />
                </div>
            </Modal.Body>
            <Modal.Actions>
                <form method="dialog" className="flex gap-2">
                    <Button onClick={onConfirmCascadeURL}>Confirm</Button>
                    <Button>Cancel</Button>
                </form>
            </Modal.Actions>
        </Modal>
    );
});
