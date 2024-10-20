import { useState } from 'preact/hooks';
import { TargetedEvent } from 'preact/compat';

import * as livemanApi from '../api';
import * as sharedApi from '../../shared/api';
import { alertError } from '../../shared/utils';

function useInput(label: string, type = 'text') {
    const [value, setValue] = useState('');

    const inputElement = (
        <div>
            <label>
                <span class="inline-block min-w-24 font-bold">{label}</span>
                <input type={type} value={value} onInput={e => setValue(e.currentTarget?.value)} />
            </label>
        </div>
    );

    return [value, inputElement] as const;
}

enum AuthorizeType {
    Password = 'Password', Token = 'Token'
}

export interface LoginProps {
    onSuccess?: (token: string) => void;
}

export function Login({ onSuccess }: LoginProps) {
    const [username, usernameInput] = useInput('Username');
    const [password, passwordInput] = useInput('Password', 'password');

    const [token, tokenInput] = useInput('Token');

    const [authType, setAuthType] = useState(AuthorizeType.Password);
    const onAuthTypeInput = (e: TargetedEvent<HTMLInputElement>) => {
        setAuthType(e.currentTarget.value as AuthorizeType);
    };

    const onLoginSubmit = async (e: TargetedEvent) => {
        e.preventDefault();
        try {
            const res = await livemanApi.login(username, password);
            const tk = `${res.token_type} ${res.access_token}`;
            livemanApi.setAuthToken(tk);
            sharedApi.setAuthToken(tk);
            onSuccess?.(res.access_token);
        } catch (e) {
            alertError(e);
        }
    };

    const onTokenSubmit = async (e: TargetedEvent) => {
        e.preventDefault();
        const tk = token.indexOf(' ') < 0 ? `Bearer ${token}` : token;
        livemanApi.setAuthToken(tk);
        sharedApi.setAuthToken(tk);
        try {
            await livemanApi.getNodes();
            onSuccess?.(token);
        } catch (e) {
            livemanApi.setAuthToken('');
            sharedApi.setAuthToken('');
            alertError(e);
        }
    };

    return (
        <fieldset>
            <legend>Authorization Required</legend>
            <div>
                <span>Authorize Type:</span>
                {[AuthorizeType.Password, AuthorizeType.Token].map(t => (
                    <label>
                        <input type="radio" name="authorizeType" value={t} checked={authType === t} onInput={onAuthTypeInput} />
                        <span>{t}</span>
                    </label>
                ))}
            </div>
            {authType === AuthorizeType.Password ? (
                <form onSubmit={onLoginSubmit}>
                    {usernameInput}
                    {passwordInput}
                    <input type="submit" value="Login" />
                </form>
            ) : authType === AuthorizeType.Token ? (
                <form onSubmit={onTokenSubmit}>
                    {tokenInput}
                    <input type="submit" value="Login" />
                </form>
            ) : null}
        </fieldset>
    );
}
