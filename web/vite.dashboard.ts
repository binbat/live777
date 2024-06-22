import { resolve } from 'node:path'

import { defineConfig } from 'vite'
import preact from '@preact/preset-vite'
import unocss from 'unocss/vite'

// https://vitejs.dev/config/
export default defineConfig({
    root: 'web',
    server: {
        proxy: {
            '^/api/.*': 'http://localhost:8888',
            '^/whip/.*': 'http://localhost:8888',
            '^/whep/.*': 'http://localhost:8888',
            '^/session/.*': 'http://localhost:8888',
        },
    },
    build: {
        outDir: '../dashboard/',
        rollupOptions: {
            input: {
                index: resolve(__dirname, 'index.html'),
                player: resolve(__dirname, 'player.html'),
                debugger: resolve(__dirname, 'debugger.html')
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
