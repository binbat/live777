import { resolve } from 'node:path'

import { defineConfig } from 'vite'
import preact from '@preact/preset-vite'
import unocss from 'unocss/vite'

export const ProjectRoot = resolve(import.meta.dirname, '..')

/**
 * shared vite config
 * @see https://vitejs.dev/config/
 */
export default defineConfig({
    publicDir: resolve(ProjectRoot, 'public'),
    server: {
        proxy: {
            '^.*/admin/.*': 'http://localhost:7777',
            '^/api/.*': 'http://localhost:7777',
            '^/session/.*': 'http://localhost:7777',
            '^/whip/.*': 'http://localhost:7777',
            '^/whep/.*': 'http://localhost:7777',
        },
    },
    build: {
        emptyOutDir: true,
    },
    plugins: [
        preact(),
        unocss({
            configFile: resolve(ProjectRoot, 'web/uno.config.ts')
        }),
    ],
})
