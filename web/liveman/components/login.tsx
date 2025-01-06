import { useEffect, useRef, useState } from 'preact/hooks';
import { TargetedEvent } from 'preact/compat';
import { WretchError } from 'wretch/resolver';
import { Alert, Button, Loading, Modal, Tabs } from 'react-daisyui';

import * as livemanApi from '../api';
import * as sharedApi from '@/shared/api';

enum AuthorizeType {
    Password = 'Password',
    Token = 'Token'
}

function useInput(label: string, type = 'text') {
    const [value, setValue] = useState('');

    const inputElement = (
        <label class="input input-bordered flex items-center gap-2 my-4">
            <span>{label}</span>
            <input type={type} class="grow" name={label} value={value} onInput={e => setValue(e.currentTarget?.value)} />
        </label>
    );

    return [value, inputElement] as const;
}

export interface LoginProps {
    show: boolean;
    onSuccess?: (tokenType: string, tokenValue: string) => void;
}

export function Login({ show, onSuccess }: LoginProps) {
    const refDialog = useRef<HTMLDialogElement>(null);
    const [authType, setAuthType] = useState(AuthorizeType.Password);
    const [username, usernameInput] = useInput('Username');
    const [password, passwordInput] = useInput('Password', 'password');
    const [token, tokenInput] = useInput('Token');
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

    const handleLogin = async (e: TargetedEvent) => {
        setErrMsg(null);
        setLoading(true);
        e.preventDefault();
        try {
            let tokenType, tokenValue;
            switch (authType) {
                case AuthorizeType.Password: {
                    const res = await livemanApi.login(username, password);
                    tokenType = res.token_type;
                    tokenValue = res.access_token;
                    break;
                }
                case AuthorizeType.Token: {
                    tokenType = 'Bearer';
                    tokenValue = token;
                    livemanApi.setAuthToken(`${tokenType} ${tokenValue}`);
                    await livemanApi.getNodes();
                    break;
                }
            }
            const tk = `${tokenType} ${tokenValue}`;
            livemanApi.setAuthToken(tk);
            sharedApi.setAuthToken(tk);
            onSuccess?.(tokenType, tokenValue);
        } catch (e) {
            livemanApi.setAuthToken('');
            sharedApi.setAuthToken('');
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
            <Modal.Header className="mb-2">
                <h3 className="font-bold">Authorization Required</h3>
            </Modal.Header>
            <Tabs variant="bordered" size="lg" className="my-4">
                {Object.values(AuthorizeType).map(t =>
                    <Tabs.Tab className="text-base" active={t === authType} onClick={() => setAuthType(t)}>{t}</Tabs.Tab>
                )}
            </Tabs>
            {typeof errMsg === 'string' ? <Alert status="error" >{errMsg}</Alert> : null}
            <form onSubmit={handleLogin}>
                {authType === AuthorizeType.Password ? [usernameInput, passwordInput]
                    : authType === AuthorizeType.Token ? tokenInput
                        : null}
                <Button color="primary" className="w-full text-base" disabled={loading}>
                    {loading ? <Loading size="sm" /> : null}
                    <span>Login</span>
                </Button>
            </form>
        </Modal>
    );
}
