import { useState } from 'preact/hooks';

import { login } from './api';

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

export interface LoginProps {
    onSuccess?: () => void;
}

export function Login({ onSuccess }: LoginProps) {
    const [username, usernameInput] = useInput('Username');
    const [password, passwordInput] = useInput('Password', 'password');

    const onLoginClick = async () => {
        login(username, password);
        onSuccess?.();
    };

    return (
        <fieldset>
            <legend>Authorization Required</legend>
            {usernameInput}
            {passwordInput}
            <button onClick={onLoginClick}>Login</button>
        </fieldset>
    );
}
