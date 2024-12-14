import { type PropsWithChildren } from 'preact/compat';
import { PageHeader } from './page-header';

import { type ITokenContext, TokenContext } from '../context';

export type PageLayoutProps = PropsWithChildren<ITokenContext>;

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
