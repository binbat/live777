import { resolve } from "node:path";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

export default defineConfig({
    plugins: [vue({ features: { vapor: true } })],
    resolve: {
        alias: [
            {
                find: /^@binbat\/whep-player-vue$/,
                replacement: "@binbat/whep-player-vue/vapor",
            },
            {
                find: "@",
                replacement: resolve(__dirname, ".."),
            },
        ],
    },
    build: {
        lib: {
            entry: resolve(__dirname, "main.ts"),
            fileName: "index",
            formats: ["es"],
        },
        rollupOptions: {
            external: ["vue"],
        },
        minify: true,
        outDir: "dist",
    },
    server: {
        proxy: {
            "^.*/admin/.*": "http://localhost:7777",
            "^/api/.*": "http://localhost:7777",
            "^/session/.*": "http://localhost:7777",
            "^/whip/.*": "http://localhost:7777",
            "^/whep/.*": "http://localhost:7777",
        },
    },
});
