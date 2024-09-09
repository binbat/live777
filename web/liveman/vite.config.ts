import { resolve } from 'node:path';

import { defineConfig, mergeConfig } from 'vite';

import CommonConfig, { ProjectRoot } from '../vite.config';

// directory name of the current module (web/liveman)
const configDir = import.meta.dirname;

// https://vitejs.dev/config/
export default mergeConfig(CommonConfig, defineConfig({
    root: configDir,
    server: {
        proxy: {
            '^/whip/.*': 'http://localhost:8888',
            '^/whep/.*': 'http://localhost:8888',
            '^/session/.*': 'http://localhost:8888',
            '^/api/.*': 'http://localhost:8888',
            '^/login$': 'http://localhost:8888',
        },
    },
    build: {
        outDir: resolve(ProjectRoot, 'assets/liveman'),
        rollupOptions: {
            input: {
                index: resolve(configDir, 'index.html'),
                player: resolve(configDir, 'tools/player.html'),
                debugger: resolve(configDir, 'tools/debugger.html'),
            }
        }
    }
}), /* isRoot = */ false);
