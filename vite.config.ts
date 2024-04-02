import { defineConfig } from 'vite'
import preact from '@preact/preset-vite'
import unocss from 'unocss/vite'

// https://vitejs.dev/config/
export default defineConfig({
    server: {
        proxy: {
            '^.*/infos': 'http://localhost:7777',
            '^/resource/.*': 'http://localhost:7777',
            '^/whip/.*': 'http://localhost:7777',
            '^/whep/.*': 'http://localhost:7777',
        },
    },
    build: {
        outDir: "gateway/assets/"
    },
    plugins: [ unocss({
        shortcuts: [
            { 'cool-blue': 'bg-blue-500 text-white' },
            { 'cool-green': 'bg-green-500 text-black' },
        ],
    }), preact()],
})