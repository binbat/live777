import { useState } from 'preact/hooks';
import { useAuth } from './components/auth';
import { Card, Alert } from 'react-daisyui';
import { KeyIcon, UserIcon } from '@heroicons/react/24/outline';

export function LoginPage(_props: { path: string }) {
    const [username, setUsername] = useState('admin');
    const [password, setPassword] = useState('');
    const [error, setError] = useState('');
    const [isLoading, setIsLoading] = useState(false);
    const { login } = useAuth();

    const handleSubmit = async (e: Event) => {
        e.preventDefault();
        setError('');
        setIsLoading(true);

        try {
            const response = await fetch('/api/login', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ username, password }),
            });

            const data = await response.json();

            if (response.ok) {
                login(data.token);
            } else {
                setError(data.error_description || data.error || 'username or password error');
            }
        } catch (err) {
            console.error('Login request failed:', err);
            setError('login failed, please check the network or server status');
        } finally {
            setIsLoading(false);
        }
    };

    return (
        <div className="min-h-screen flex items-center justify-center bg-base-200 p-4">
            <Card className="w-full max-w-md bg-base-100 shadow-xl">
                <Card.Body>
                    <Card.Title className="text-center text-2xl mb-4">Livecam login</Card.Title>
                    <form onSubmit={handleSubmit}>
                        <div className="form-control">
                            <label className="label">
                                <span className="label-text">username</span>
                            </label>
                            <label className="input input-bordered flex items-center gap-2">
                                <UserIcon className="w-4 h-4" />
                                <input
                                    type="text"
                                    className="grow"
                                    placeholder="Username"
                                    value={username}
                                    onInput={(e) => setUsername(e.currentTarget.value)}
                                    disabled={isLoading}
                                />
                            </label>
                        </div>
                        <div className="form-control mt-4">
                            <label className="label">
                                <span className="label-text">password</span>
                            </label>
                            <label className="input input-bordered flex items-center gap-2">
                                <KeyIcon className="w-4 h-4" />
                                <input
                                    type="password"
                                    className="grow"
                                    placeholder="Password"
                                    value={password}
                                    onInput={(e) => setPassword(e.currentTarget.value)}
                                    disabled={isLoading}
                                />
                            </label>
                        </div>

                        {error && (
                            <Alert status="error" className="mt-4">
                                <span>{error}</span>
                            </Alert>
                        )}

                        <div className="form-control mt-6">
                            <button
                                type="submit"
                                className="btn btn-primary w-full"
                                disabled={isLoading || !username || !password}
                            >
                                {isLoading && <span className="loading loading-spinner"></span>}
                                {isLoading ? 'logging in...' : 'login'}
                            </button>
                        </div>
                    </form>
                </Card.Body>
            </Card>
        </div>
    );
}
