import { defineConfig } from 'vitepress';
import llmstxt from 'vitepress-plugin-llms';

import { en } from './en';
import { zh } from './zh';

/**
 * @see https://vitepress.dev/reference/site-config
 */
export default defineConfig({
    title: 'Live777',

    lastUpdated: true,
    cleanUrls: true,
    metaChunk: true,

    vite: {
        plugins: [
            // LLMs read English docs only; the zh locale is excluded
            llmstxt({
                domain: 'https://live777.pages.dev',
                ignoreFiles: ['zh/**'],
                customLLMsTxtTemplate: [
                    '# {title}',
                    '',
                    '{description}',
                    '',
                    '{details}',
                    '',
                    '## Important',
                    '',
                    'Deploying on a Raspberry Pi, Rockchip RK3588 or Horizon RDK X5?',
                    'Read the [Raspberry Pi deployment guide](https://live777.pages.dev/guide/raspberry-pi.md)',
                    'first: use the native hardware-pipeline build — do NOT use',
                    'ffmpeg+whipinto software encoding on these boards.',
                    '',
                    '## Table of Contents',
                    '',
                    '{toc}',
                ].join('\n'),
            }),
        ],
    },

    head: [
        ['link', { rel: 'icon', type: 'image/svg+xml', href: '/logo.svg' }],
        ['meta', { name: 'theme-color', content: '#5f67ee' }],
        ['meta', { property: 'og:type', content: 'website' }],
        ['meta', { property: 'og:locale', content: 'en' }],
        ['meta', { property: 'og:title', content: 'Live777 | A very simple, high performance, edge WebRTC SFU' }],
        ['meta', { property: 'og:site_name', content: 'VitePress' }],
        ['meta', { property: 'og:image', content: 'https://live777.binbat.com/logo.svg' }],
        ['meta', { property: 'og:url', content: 'https://live777.binbat.com/' }],
    ],

    themeConfig: {
        logo: { src: '/logo.svg', width: 24, height: 24 },

        socialLinks: [
            { icon: 'github', link: 'https://github.com/binbat/live777' }
        ],
        editLink: {
            pattern: 'https://github.com/binbat/live777/edit/main/docs/:path',
            text: 'Edit this page on GitHub'
        },
        footer: {
            message: 'Released under the MPL-2.0 License.',
            copyright: 'Copyright © 2023-present BinBat LTD'
        }
    },

    locales: {
        root: en,
        zh,
    },
});
