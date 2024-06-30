import { resolve } from 'node:path'

import { defineConfig, mergeConfig } from 'vite'

import CommonConfig, { ProjectRoot } from '../vite.config'

// directory name of the current module (web/liveman)
const configDir = import.meta.dirname

// https://vitejs.dev/config/
export default mergeConfig(CommonConfig, defineConfig({
    root: configDir,
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
}), /* isRoot = */ false)
