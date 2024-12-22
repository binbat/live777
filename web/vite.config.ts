import { resolve } from 'node:path';

import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';
import tailwindcss from 'tailwindcss';
import daisyui from 'daisyui';

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
    },
    css: {
        postcss: {
            plugins: [
                tailwindcss({
                    content: [
                        'node_modules/daisyui/dist/**/*.js',
                        'node_modules/react-daisyui/dist/**/*.js',
                        'web/**/*.{html,tsx}'
                    ].map(p => resolve(ProjectRoot, p)),
                    plugins: [
                        daisyui
                    ],
                })
            ]
        }
    }
});
