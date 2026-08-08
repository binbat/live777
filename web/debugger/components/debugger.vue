<script setup lang="ts">
import "./debugger.css";
import { useSearchParams } from "../primitive/search-params";
import Publisher from "./publisher.vue";
import Subscriber from "./subscriber.vue";

const [searchParams, setSearchParams] = useSearchParams();

const onStreamIdInput = (event: Event) => {
    setSearchParams({ id: (event.target as HTMLInputElement).value });
};

const onTokenInput = (event: Event) => {
    setSearchParams({ token: (event.target as HTMLInputElement).value });
};
</script>

<template>
    <fieldset>
        <legend>Common</legend>
        <section style="display: flex; justify-content: space-evenly; flex-wrap: wrap">
            <div>
                Stream ID:
                <input type="text" :value="searchParams.id || ''" @input="onStreamIdInput" />
            </div>
            <div>
                Bearer Token:
                <input type="text" :value="searchParams.token || ''" @input="onTokenInput" />
            </div>
        </section>
    </fieldset>
    <div style="display: flex; justify-content: space-evenly; flex-wrap: wrap">
        <fieldset>
            <Publisher />
        </fieldset>
        <fieldset>
            <Subscriber />
        </fieldset>
    </div>
</template>
