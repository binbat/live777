import { Router } from 'preact-router';
import { AuthProvider } from './components/auth';
import { ProtectedRoute } from './components/protected-route';
import { LoginPage } from './login-page';
import { LiveCamPage } from './livecam';

export function App() {
    return (
        <AuthProvider>
            <Router>
                <LoginPage path="/login" />

                <ProtectedRoute path="/" component={LiveCamPage} />
            </Router>
        </AuthProvider>
    );
}
