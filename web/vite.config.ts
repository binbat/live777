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
