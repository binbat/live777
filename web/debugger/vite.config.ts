import { resolve } from "node:path";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
    plugins: [solid()],
    resolve: {
        alias: {
            "@": resolve(__dirname, ".."),
        },
    },
    build: {
        lib: {
            entry: resolve(__dirname, "main.tsx"),
            fileName: "index",
            formats: ["es"],
        },
        rollupOptions: {
            external: ["solid-js", "solid-js/web"],
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
