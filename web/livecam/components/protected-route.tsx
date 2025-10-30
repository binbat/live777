import { ComponentType } from 'preact';
import { route } from 'preact-router';
import { useEffect } from 'preact/hooks';
import { useAuth } from './auth';

type RouteComponent = ComponentType<{ path: string; [key: string]: unknown }>;

interface ProtectedRouteProps {
    path: string;
    component: RouteComponent;
}

export function ProtectedRoute({ component: Component, ...rest }: ProtectedRouteProps) {
    const { isLoggedIn, isLoading } = useAuth();

    useEffect(() => {
        if (!isLoading && !isLoggedIn) {
            route('/login', true);
        }
    }, [isLoggedIn, isLoading]);

    if (isLoading) {
        return (
            <div className="min-h-screen flex items-center justify-center bg-base-200">
                <span className="loading loading-lg loading-spinner text-primary"></span>
            </div>
        );
    }

    return isLoggedIn ? <Component {...rest} /> : null;
}
