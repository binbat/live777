import { useState } from 'preact/hooks';

import * as api from '@/shared/api';
import { useNeedAuthorization } from '@/shared/hooks/use-need-authorization';
import { PageLayout } from '@/shared/components/page-layout';
import { StreamsTable } from '@/shared/components/streams-table';
import { RecordingsPage } from '@/shared/components/recordings-page';

import { Login } from './components/login';

export function Liveion() {
    const [token, setToken] = useState('');
    const [needsAuthorizaiton, setNeedsAuthorization] = useNeedAuthorization(api);
    const [currentView, setCurrentView] = useState<'streams' | 'recordings'>(() => {
        const view = new URLSearchParams(location.search).get('view');
        return view === 'recordings' ? 'recordings' : 'streams';
    });

    const onLoginSuccess = (t: string) => {
        setToken(t);
        setNeedsAuthorization(false);
    };

    const navigateToView = (view: 'streams' | 'recordings') => {
        const url = new URL(window.location.href);
        if (view === 'streams') {
            url.searchParams.delete('view');
        } else {
            url.searchParams.set('view', view);
        }
        window.history.pushState({}, '', url.toString());
        setCurrentView(view);
    };

    const renderCurrentView = () => {
        if (currentView === 'recordings') {
            return <RecordingsPage />;
        }

        return (
            <StreamsTable
                showCascade
                features={{ debugger: true, player: true, recording: true, autoDetectRecording: true, recordingPlayback: true }}
            />
        );
    };

    return (
        <>
            <PageLayout
                token={token}
                currentView={currentView}
                onNavigate={(view: string) => navigateToView(view as 'streams' | 'recordings')}
                enabledTools={{ debugger: true, player: true, dash: true }}
            >
                {renderCurrentView()}
            </PageLayout>
            <Login
                show={needsAuthorizaiton}
                onSuccess={onLoginSuccess}
            />
        </>
    );
}
