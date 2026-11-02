import { resolve } from 'node:path';

import tailwindcss from '@tailwindcss/vite';
import vue from '@vitejs/plugin-vue';
import { defineConfig } from 'vite';

export const ProjectRoot = resolve(import.meta.dirname, '..');

/**
 * shared vite config
 * @see https://vitejs.dev/config/
 */
export default defineConfig({
    publicDir: resolve(ProjectRoot, 'web/public'),
    build: {
        emptyOutDir: true,
    },
    plugins: [
        tailwindcss(),
        vue()
    ],
    resolve: {
        alias: {
            '@': resolve(ProjectRoot, 'web')
        }
    }
});
