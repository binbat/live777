import { useState } from 'preact/hooks';

import * as api from '@/shared/api';
import { useNeedAuthorization } from '@/shared/hooks/use-need-authorization';
import { PageLayout } from '@/shared/components/page-layout';
import { StreamsTable } from '@/shared/components/streams-table';

import { Login } from './components/login';

export function Liveion() {
    const [token, setToken] = useState('');
    const [needsAuthorizaiton, setNeedsAuthorization] = useNeedAuthorization(api);

    const onLoginSuccess = (t: string) => {
        setToken(t);
        setNeedsAuthorization(false);
    };

    return (
        <>
            <PageLayout
                token={token}
                enabledTools={{ debugger: true, player: true, dash: false }}
            >
                <StreamsTable
                    showCascade
                    features={{ debugger: true, player: true, recording: true, autoDetectRecording: true, recordingPlayback: false }}
                />
            </PageLayout>
            <Login
                show={needsAuthorizaiton}
                onSuccess={onLoginSuccess}
            />
        </>
    );
}
