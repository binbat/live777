import { resolve } from 'node:path';

import { defineConfig, mergeConfig } from 'vite';

import CommonConfig, { ProjectRoot } from '../vite.config';

// directory name of the current module (web/liveion)
const configDir = import.meta.dirname;

// https://vitejs.dev/config/
export default mergeConfig(CommonConfig, defineConfig({
    root: configDir,
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
        outDir: resolve(ProjectRoot, 'assets/liveion'),
        rollupOptions: {
            input: {
                index: resolve(configDir, 'index.html'),
                player: resolve(configDir, 'tools/player.html'),
                debugger: resolve(configDir, 'tools/debugger.html'),
            }
        }
    }
}), /* isRoot = */ false);
