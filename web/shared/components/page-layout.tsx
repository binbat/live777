import { type PropsWithChildren } from 'preact/compat';
import { PageHeader } from './page-header';

import { type ITokenContext, TokenContext } from '../context';

export interface PageLayoutProps extends PropsWithChildren<ITokenContext> {
    currentView?: string;
    onNavigate?: (view: string) => void;
}

export function PageLayout({ token, currentView, onNavigate, children }: PageLayoutProps) {
    return (
        <TokenContext.Provider value={{ token }}>
            <PageHeader currentView={currentView} onNavigate={onNavigate} />
            <div className="max-w-screen-xl mx-auto">
                {children}
            </div>
        </TokenContext.Provider>
    );
}
