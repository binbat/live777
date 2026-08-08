import { inject, provide, ref, type InjectionKey, type Ref } from "vue";

export interface ITokenContext {
    token: string;
}

export const TokenContextKey: InjectionKey<Ref<string>> = Symbol("TokenContext");

/**
 * Provide the current auth token to the component subtree.
 * Call once in the setup of a layout/root component (e.g. PageLayout);
 * the token stays reactive when the passed ref changes.
 */
export function provideToken(token: Ref<string>): Ref<string> {
    provide(TokenContextKey, token);
    return token;
}

/**
 * Inject the token provided by the nearest `provideToken` ancestor.
 * Falls back to an empty-token ref outside a provider, matching the old
 * `TokenContext` default value `{ token: '' }`.
 */
export function useToken(): Ref<string> {
    return inject(TokenContextKey, () => ref(""), true);
}
