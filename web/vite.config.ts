import { resolve } from 'node:path';

import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import tailwindcss from 'tailwindcss';
import daisyui from 'daisyui';

export const ProjectRoot = resolve(import.meta.dirname, '..');

const workspaceContentRoots = [
    'web/shared',
    'web/debugger'
];

const defaultAppContentRoots = [
    'web/liveion',
    'web/liveman'
];

const packageName = process.env.npm_package_name;

const appContentRoots = packageName
    ? [`web/${packageName}`]
    : defaultAppContentRoots;

const tailwindContentGlobs = [
    'node_modules/daisyui/dist/**/*.js',
    ...appContentRoots.flatMap(root => [
        `${root}/index.html`,
        `${root}/**/*.{ts,tsx,vue,html}`
    ]),
    ...workspaceContentRoots.flatMap(root => [
        `${root}/**/*.{ts,tsx,vue,html}`
    ])
]
    .map(p => resolve(ProjectRoot, p));

/**
 * shared vite config
 * @see https://vitejs.dev/config/
 */
export default defineConfig({
    publicDir: resolve(ProjectRoot, 'web/public'),
    build: {
        emptyOutDir: true,
    },
    plugins: [
        vue()
    ],
    resolve: {
        alias: {
            '@': resolve(ProjectRoot, 'web')
        }
    },
    css: {
        postcss: {
            plugins: [
                tailwindcss({
                    content: tailwindContentGlobs,
                    plugins: [
                        daisyui
                    ],
                })
            ]
        }
    }
});
