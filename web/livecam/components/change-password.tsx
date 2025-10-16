import { forwardRef, useImperativeHandle, useRef, useState } from 'preact/compat';
import { Modal, Button, Alert } from 'react-daisyui';
import { KeyIcon } from '@heroicons/react/24/outline';
import { useAuth } from './auth';

export interface ChangePasswordRef {
    show: () => void;
}

interface ChangePasswordProps {
    className?: string; 
}

export const ChangePassword = forwardRef<ChangePasswordRef, ChangePasswordProps>((_props, ref) => {
    const modalRef = useRef<HTMLDialogElement>(null);
    const { token } = useAuth(); 

    const [newPassword, setNewPassword] = useState('');
    const [isLoading, setIsLoading] = useState(false);
    const [error, setError] = useState('');
    const [success, setSuccess] = useState('');

    useImperativeHandle(ref, () => ({
        show: () => {
            setNewPassword('');
            setError('');
            setSuccess('');
            modalRef.current?.showModal();
        }
    }));

    const handleSubmit = async () => {
        if (!newPassword) {
            setError('Password cannot be empty.');
            return;
        }
        setIsLoading(true);
        setError('');
        setSuccess('');

        try {
            const response = await fetch('/api/user/password', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                    'Authorization': `Bearer ${token}`
                },
                body: JSON.stringify({ new_password: newPassword })
            });

            if (response.ok) {
                setSuccess('Password updated successfully!');
                setNewPassword(''); 
                setTimeout(() => modalRef.current?.close(), 3000);
            } else {
                const data = await response.json();
                setError(data.message || 'Failed to update password. Please try again.');
            }
        } catch (err) {
            console.error('Change password request failed:', err);
            setError('An unexpected network error occurred.');
        } finally {
            setIsLoading(false);
        }
    };

    return (
        <Modal ref={modalRef}>
            <Modal.Header className="font-bold">Change Password</Modal.Header>
            <Modal.Body>
                <div className="form-control mt-4">
                    <label className="label">
                        <span className="label-text">New Password</span>
                    </label>
                    <label className="input input-bordered flex items-center gap-2">
                        <KeyIcon className="w-4 h-4" />
                        <input
                            type="password"
                            className="grow"
                            placeholder="Enter new password"
                            value={newPassword}
                            onInput={(e) => setNewPassword(e.currentTarget.value)}
                            disabled={isLoading}
                        />
                    </label>
                </div>

                {error && (
                    <Alert status="error" className="mt-4">
                        <span>{error}</span>
                    </Alert>
                )}
                {success && (
                    <Alert status="success" className="mt-4">
                        <span>{success}</span>
                    </Alert>
                )}

            </Modal.Body>
            <Modal.Actions>
                <Button onClick={() => modalRef.current?.close()} disabled={isLoading}>Cancel</Button>
                <Button color="primary" onClick={handleSubmit} disabled={isLoading || !newPassword}>
                    {isLoading && <span className="loading loading-spinner"></span>}
                    {isLoading ? 'Saving...' : 'Save'}
                </Button>
            </Modal.Actions>
        </Modal>
    );
});
