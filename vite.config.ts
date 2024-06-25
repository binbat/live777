import { resolve } from 'node:path'

import { defineConfig } from 'vite'
import preact from '@preact/preset-vite'
import unocss from 'unocss/vite'

// https://vitejs.dev/config/
export default defineConfig({
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
        outDir: 'assets/',
        rollupOptions: {
            input: {
                index: resolve(__dirname, 'index.html'),
                player: resolve(__dirname, 'web/player.html'),
                debugger: resolve(__dirname, 'web/debugger.html')
            }
        }
    },
    plugins: [unocss({
        rules: [
            [/^mw-([\.\d]+)$/, ([_, num]) => ({ 'min-width': `${num}px` })],
        ],
        shortcuts: [
            { 'cool-blue': 'bg-blue-500 text-white' },
            { 'cool-green': 'bg-green-500 text-black' },
        ],
    }), preact()],
})
