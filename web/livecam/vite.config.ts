
import { resolve } from 'node:path';
import { defineConfig, mergeConfig } from 'vite';


import CommonConfig, { ProjectRoot } from '../vite.config';


const configDir = import.meta.dirname;

// https://vitejs.dev/config/
export default mergeConfig(CommonConfig, defineConfig({
    root: configDir,

    server: {
        proxy: {
            '^.*/admin/.*': 'http://localhost:9999',
            '^/api/.*': 'http://localhost:9999',
            '^/session/.*': 'http://localhost:9999',
            '^/whep/.*': 'http://localhost:9999',
        },
    },

    build: {
        outDir: resolve(ProjectRoot, 'assets/livecam'),
        rollupOptions: {
            input: {
                index: resolve(configDir, 'index.html'),
            }
        }
    }
}), /* isRoot = */ false);
