import { resolve } from "node:path";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

// Dual build: default VDOM output (dist/, Vue 3.5+) and a Vapor-compiled
// variant (dist/vapor/, Vue 3.6+) from the same sources. The Vapor build
// runs second and must not empty the shared outDir.
export default defineConfig(({ mode }) => {
    const vapor = mode === "vapor";
    return {
        plugins: [vue({ features: { vapor } })],
        build: {
            lib: {
                entry: resolve(__dirname, "index.ts"),
                fileName: "index",
                formats: ["es"],
            },
            rollupOptions: {
                external: ["vue"],
            },
            minify: true,
            outDir: vapor ? "dist/vapor" : "dist",
            emptyOutDir: !vapor,
        },
    };
});
