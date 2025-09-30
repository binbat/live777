import { createContext } from 'preact';
import { useContext, useState, useEffect } from 'preact/hooks';
import { route } from 'preact-router';
import { useCallback } from 'preact/compat';

interface AuthContextType {
  token: string | null;
  isLoggedIn: boolean;
  isLoading: boolean;
  login: (token: string) => void;
  logout: () => void;
}

export const AuthContext = createContext<AuthContextType>({
  token: null,
  isLoggedIn: false,
  isLoading: true, 
  login: () => {},
  logout: () => {},
});

export const AuthProvider = ({ children }: { children: preact.ComponentChildren }) => {
  const [token, setToken] = useState<string | null>(null); 
  const [isLoading, setIsLoading] = useState(true);


  const logout = useCallback(() => {
    localStorage.removeItem('authToken');
    setToken(null);
    if (window.location.pathname !== '/login') {
      route('/login', true);
    }
  }, []);

  useEffect(() => {
    const verifySession = async () => {
      const storedToken = localStorage.getItem('authToken');
      if (!storedToken) {
        setIsLoading(false);
        return;
      }

      try {
        const res = await fetch('/api/session', {
          method: 'POST',
          headers: { Authorization: `Bearer ${storedToken}` },
        });
        if (res.ok) {
          setToken(storedToken);
        } else {
          logout();
        }
      } catch (error) {
        console.error('session verification request failed', error);
        logout();
      } finally {
        setIsLoading(false);
      }
    };

    verifySession();
  }, [logout]); 

  const login = (newToken: string) => {
    localStorage.setItem('authToken', newToken);
    setToken(newToken);
    route('/', true);
  };

  const value = {
    token,
    isLoggedIn: !!token,
    isLoading,
    login,
    logout,
  };

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
};

export const useAuth = () => useContext(AuthContext);
