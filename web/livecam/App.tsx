import { Router } from 'preact-router';
import { AuthProvider } from './components/AuthContext';
import { ProtectedRoute } from './components/ProtectedToute'; 
import { LoginPage } from './LoginPage';
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
