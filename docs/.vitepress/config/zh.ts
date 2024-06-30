
import { defineConfig } from 'vitepress'

export const zh = defineConfig({
    lang: 'zh-Hans',
    description: '简单，高性能，WebRTC SFU',

    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Examples', link: '/markdown-examples' }
        ],


        sidebar: {
            '/zh/guide/': { base: '/zh/guide/', items: [
                {
                    text: '简介',
                    collapsed: false,
                    items: [
                        { text: '什么是 Live777？', link: 'what-is-live777' },
                        { text: '快速开始', link: 'getting-started' },
                    ]
                },
            ]
            },
        },
    }
})
