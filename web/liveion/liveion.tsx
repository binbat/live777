import { useEffect, useState } from 'preact/hooks';

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
    const [recorderAvailable, setRecorderAvailable] = useState(false);

    useEffect(() => {
        let disposed = false;
        (async () => {
            const status = await api.probeRecorderFeature();
            if (!disposed) {
                setRecorderAvailable(status === 'available');
            }
        })();

        return () => {
            disposed = true;
        };
    }, [token]);

    useEffect(() => {
        if (recorderAvailable || currentView !== 'recordings') {
            return;
        }

        const url = new URL(window.location.href);
        url.searchParams.delete('view');
        window.history.replaceState({}, '', url.toString());
        setCurrentView('streams');
    }, [currentView, recorderAvailable]);

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
        if (recorderAvailable && currentView === 'recordings') {
            return <RecordingsPage />;
        }

        return (
            <StreamsTable
                showCascade
                features={{ debugger: true, player: true, recording: recorderAvailable, autoDetectRecording: false, recordingPlayback: recorderAvailable }}
            />
        );
    };

    return (
        <>
            <PageLayout
                token={token}
                currentView={currentView}
                onNavigate={(view: string) => navigateToView(view as 'streams' | 'recordings')}
                enabledTools={{ debugger: true, player: true, dash: recorderAvailable, recordings: recorderAvailable }}
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
