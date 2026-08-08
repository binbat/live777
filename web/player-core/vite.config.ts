import { resolve } from "node:path";
import { defineConfig } from "vite";

export default defineConfig({
    build: {
        lib: {
            entry: resolve(__dirname, "index.ts"),
            fileName: "index",
            formats: ["es"],
        },
        minify: true,
        outDir: "dist",
    },
});
