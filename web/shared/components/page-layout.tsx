import { PropsWithChildren } from 'preact/compat';
import { PageHeader } from './page-header';

import { ITokenContext, TokenContext } from '../context';

export type PageLayoutProps = ITokenContext & PropsWithChildren;

export function PageLayout({ token, children }: PageLayoutProps) {
    return (
        <TokenContext.Provider value={{ token }}>
            <PageHeader />
            <div className="max-w-screen-xl mx-auto">
                {children}
            </div>
        </TokenContext.Provider>
    );
}
