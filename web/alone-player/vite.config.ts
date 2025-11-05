import { resolve } from "node:path";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
    plugins: [solid()],
    build: {
        lib: {
            entry: resolve(__dirname, "main.tsx"),
            fileName: "index",
            formats: ["es"],
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
