import { resolve } from 'node:path';

import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';

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
        preact()
    ],
    resolve: {
        alias: {
            '@': resolve(ProjectRoot, 'web')
        }
    }
});
