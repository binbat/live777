import { useEffect, useRef, useState } from 'preact/hooks';
import { TargetedEvent } from 'preact/compat';
import { Alert, Button, Modal } from 'react-daisyui';
import { WretchError } from 'wretch/resolver';

import * as api from '@/shared/api';

export interface LoginProps {
    show: boolean;
    onSuccess?: (token: string) => void;
}

export function Login({ show, onSuccess }: LoginProps) {
    const refDialog = useRef<HTMLDialogElement>(null);
    const [token, setToken] = useState('');
    const [loading, setLoading] = useState(false);
    const [errMsg, setErrMsg] = useState<string | null>(null);

    useEffect(() => {
        if (show) {
            refDialog.current?.showModal();
        } else {
            refDialog.current?.close();
        }
    }, [show]);

    const handleDialogClose = () => {
        if (show) {
            refDialog.current?.showModal();
        }
    };

    const onTokenSubmit = async (e: TargetedEvent) => {
        setLoading(true);
        e.preventDefault();
        const tk = token.indexOf(' ') < 0 ? `Bearer ${token}` : token;
        api.setAuthToken(tk);
        try {
            await api.getStreams();
            onSuccess?.(token);
        } catch (e) {
            api.setAuthToken('');
            if (e instanceof WretchError) {
                setErrMsg(e.json?.error ?? e.text ?? `Status: ${e.status}`);
            } else if (e instanceof Error) {
                setErrMsg(e.message);
            } else {
                setErrMsg(String(e));
            }
        }
        setLoading(false);
    };

    return (
        <Modal ref={refDialog} onClose={handleDialogClose}>
            <Modal.Header>
                <h3 className="font-bold">Authorization Required</h3>
            </Modal.Header>
            {typeof errMsg === 'string' ? <Alert status="error" >{errMsg}</Alert> : null}
            <form onSubmit={onTokenSubmit}>
                <label class="input input-bordered flex items-center gap-2 my-4">
                    <span>Token</span>
                    <input class="grow" value={token} onInput={e => setToken(e.currentTarget?.value)} />
                </label>
                <Button type="submit" color="primary" className="w-full text-base" disabled={loading}>
                    {/* @ts-expect-error -- size */}
                    {loading ? <Loading size="sm" /> : null}
                    <span>Login</span>
                </Button>
            </form>
        </Modal>
    );
}
