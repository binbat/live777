import { createContext } from 'preact';

export interface ITokenContext {
    token: string
}

export const TokenContext = createContext<ITokenContext>({ token: '' });
