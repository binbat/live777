import { useState, useRef, useImperativeHandle, useEffect } from 'preact/hooks';
import { forwardRef } from 'preact/compat';
import { Button, Checkbox, Input, Modal } from 'react-daisyui';

import { createStreamToken } from '../api';

enum StreamTokenPermission {
    Sub = 'subscribe',
    Pub = 'publish',
    Admin = 'admin'
}

export interface IStreamTokenDialog {
    show(id: string): void
}

export const StreamTokenDialog = forwardRef<IStreamTokenDialog>((_, ref) => {
    const [streamId, setStreamId] = useState('');
    const [duration, setDuration] = useState(3600);
    const [permissions, setPermissions] = useState<Record<StreamTokenPermission, boolean>>({
        subscribe: true,
        publish: false,
        admin: false
    });
    const [token, setToken] = useState('');
    const refDialog = useRef<HTMLDialogElement>(null);
    const refTokenResult = useRef<HTMLInputElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: (id: string) => {
                setStreamId(id);
                refDialog.current?.showModal();
            }
        };
    });

    const onConfirm = async (e: Event) => {
        e.preventDefault();
        const res = await createStreamToken({
            id: streamId,
            duration: duration,
            ...permissions
        });
        setToken(`${res.token_type} ${res.access_token}`);
    };

    useEffect(() => {
        if (token) {
            refTokenResult.current?.focus();
            refTokenResult.current?.select();
        }
    }, [token]);

    const onClose = (_e: Event) => {
        setToken('');
    };

    return (
        <Modal ref={refDialog} className="max-w-md" onClose={onClose}>
            <Modal.Header className="mb-2">
                <h3 className="font-bold">Create Token for stream {streamId}</h3>
            </Modal.Header>
            <Modal.Body>
                <label className="form-control">
                    <label className="label px-0">Duration:</label>
                    <label class="input input-bordered flex items-center gap-2">
                        <input
                            className="grow"
                            type="number"
                            value={duration} onInput={e => setDuration(e.currentTarget.valueAsNumber)}
                        />
                        <span>Seconds</span>
                    </label>
                </label>
                <label className="form-control mt-4">
                    <label className="label px-0 pb-0">Permissions:</label>
                    {Object.values(StreamTokenPermission).map(p =>
                        <label class="label justify-start gap-2">
                            <Checkbox
                                size="xs"
                                name={p}
                                checked={permissions[p]}
                                onChange={e => setPermissions({ ...permissions, [p]: e.currentTarget.checked })}
                            />
                            <span>{p}</span>
                        </label>
                    )}
                </label>
            </Modal.Body>
            <Modal.Actions>
                <form method="dialog" className="flex gap-2">
                    <Button onClick={onConfirm}>Confirm</Button>
                    <Button>Cancel</Button>
                </form>
            </Modal.Actions>
            {token ? (
                <label className="form-control">
                    <label className="label px-0">Token:</label>
                    <Input borderOffset type="text" value={token} ref={refTokenResult} />
                </label>
            ) : null}
        </Modal>
    );
});
