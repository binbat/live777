import preact from '@preact/preset-vite'
import unocss from 'unocss/vite'
import { defineConfig } from 'vite'

// https://vitejs.dev/config/
export default defineConfig({
    server: {
        proxy: {
            '^.*/admin/.*': 'http://localhost:7777',
            '^/resource/.*': 'http://localhost:7777',
            '^/whip/.*': 'http://localhost:7777',
            '^/whep/.*': 'http://localhost:7777',
        },
    },
    build: {
        outDir: "assets/"
    },
    plugins: [unocss({
        shortcuts: [
            { 'cool-blue': 'bg-blue-500 text-white' },
            { 'cool-green': 'bg-green-500 text-black' },
        ],
    }), preact()],
})
