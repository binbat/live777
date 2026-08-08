import { onMounted, onUnmounted, ref, type Ref } from "vue";

import { type AuthorizationCallbacks } from "../authorization-middleware";

export function useNeedAuthorization(
    auth: Omit<AuthorizationCallbacks, "setAuthorization">
): [Ref<boolean>, (value: boolean) => void] {
    const needsAuthorization = ref(false);
    const setNeedsAuthorization = (value: boolean) => {
        needsAuthorization.value = value;
    };
    const cb = () => {
        setNeedsAuthorization(true);
    };

    onMounted(() => {
        auth.addUnauthorizedCallback(cb);
    });
    onUnmounted(() => {
        auth.removeUnauthorizedCallback(cb);
    });

    return [needsAuthorization, setNeedsAuthorization];
}
