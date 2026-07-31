import { reactive } from "vue";

export type SearchParams = Record<string, string>;

export type SearchParamsInit = Record<
    string,
    string | number | null | undefined
>;

// Single reactive view of the current query string, shared by every
// component of the app (the debugger mounts as one small app, so a
// module-level store is enough).
const params = reactive<SearchParams>({});

function syncFromLocation() {
    const current = new URLSearchParams(window.location.search);
    for (const key of Object.keys(params)) {
        if (!current.has(key)) {
            delete params[key];
        }
    }
    current.forEach((value, key) => {
        params[key] = value;
    });
}

syncFromLocation();
window.addEventListener("popstate", syncFromLocation);

// Replacement for `@solidjs/router`'s `useSearchParams`, which was the only
// router feature this app used. Reading tracks the reactive params object;
// writing merges into the current query string (`null`/`undefined`/`""`
// removes the key, like the router did) and applies it with
// `history.replaceState`, so typing into the inputs never floods the
// history stack.
export function useSearchParams(): [
    SearchParams,
    (params: SearchParamsInit) => void,
] {
    const setSearchParams = (next: SearchParamsInit) => {
        const merged = new URLSearchParams(window.location.search);
        for (const [key, value] of Object.entries(next)) {
            if (value === null || value === undefined || value === "") {
                merged.delete(key);
            } else {
                merged.set(key, String(value));
            }
        }
        const search = merged.toString();
        const url = `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`;
        window.history.replaceState(null, "", url);
        syncFromLocation();
    };
    return [params, setSearchParams];
}
