import { useState, useRef, useImperativeHandle, useEffect } from 'preact/hooks';
import { forwardRef } from 'preact/compat';

import { createStreamToken } from '../api';

enum StreamTokenPermission {
    Sub = 'subscribe', Pub = 'publish', Admin = 'admin'
}

export interface INewStreamDialog {
    show(id: string): void
}

export const StreamTokenDialog = forwardRef<INewStreamDialog>((_, ref) => {
    const [streamId, setStreamId] = useState('');
    const [duration, setDuration] = useState(3600);
    const [permissions, setPermissions] = useState({ subscribe: true, publish: false, admin: false });
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
        <dialog class="min-w-96" ref={refDialog} onClose={onClose}>
            <h3>Create Token for stream {streamId}</h3>
            <p>
                <label>
                    <span>Duration: </span>
                    <br />
                    <input type="number" value={duration} onInput={e => setDuration(e.currentTarget.valueAsNumber)} />
                    <span>(seconds)</span>
                </label>
            </p>
            <p>
                <span>Permissions:</span>
                <br />
                {[StreamTokenPermission.Sub, StreamTokenPermission.Pub, StreamTokenPermission.Admin].map(p => (
                    <label>
                        <input
                            type="checkbox" name={p} checked={permissions[p]}
                            onChange={e => setPermissions({ ...permissions, [p]: e.currentTarget.checked })}
                        />
                        <span>{p}</span>
                        <br />
                    </label>
                ))}
            </p>
            <form method="dialog">
                <button>Cancel</button>
                <button onClick={onConfirm}>Confirm</button>
            </form>
            {token ? (
                <>
                    <hr />
                    <label class="flex flex-col">
                        <span>Token:</span>
                        <input type="text" class="grow" value={token} ref={refTokenResult} />
                    </label>
                </>
            ) : null}
        </dialog>
    );
});
