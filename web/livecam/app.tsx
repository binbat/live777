import { Router } from 'preact-router';
import { AuthProvider } from './components/auth';
import { ProtectedRoute } from './components/protected-route';
import { LoginPage } from './login-page';
import { LiveCamPage } from './livecam';
import { Av1Player } from './av1-player';

export function App() {
    return (
        <AuthProvider>
            <Router>
                <LoginPage path="/login" />

                <ProtectedRoute path="/" component={LiveCamPage} />
                <ProtectedRoute path="/av1" component={() => <Av1Player src="/assets/av1_samples/stream.mpd" />} />
            </Router>
        </AuthProvider>
    );
}
