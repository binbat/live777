import { useState } from 'preact/hooks';
import { TargetedEvent } from 'preact/compat';

import * as api from '../../shared/api';
import { alertError } from '../../shared/utils';

export interface LoginProps {
    onSuccess?: (token: string) => void;
}

export function Login({ onSuccess }: LoginProps) {
    const [token, setToken] = useState('');

    const onTokenSubmit = async (e: TargetedEvent) => {
        e.preventDefault();
        const tk = token.indexOf(' ') < 0 ? `Bearer ${token}` : token;
        api.setAuthToken(tk);
        try {
            await api.getStreams();
            onSuccess?.(token);
        } catch (e) {
            api.setAuthToken('');
            alertError(e);
        }
    };

    return (
        <fieldset>
            <legend>Authorization Required</legend>
            <form onSubmit={onTokenSubmit}>
                <span class="inline-block min-w-24 font-bold">Token</span>
                <input value={token} onInput={e => setToken(e.currentTarget?.value)} />
                <br />
                <input type="submit" value="Login" />
            </form>
        </fieldset>
    );
}
