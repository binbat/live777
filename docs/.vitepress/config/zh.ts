
import { defineConfig } from 'vitepress'

export const zh = defineConfig({
    lang: 'zh-Hans',
    description: '简单，高性能，WebRTC SFU',

    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: '指引', link: '/guide/what-is-live777' }
        ],


        sidebar: {
            '/zh/guide/': { base: '/zh/guide/', items: [
                {
                    text: '简介',
                    collapsed: false,
                    items: [
                        { text: '什么是 Live777 ？', link: 'what-is-live777' },
                        { text: '简介', link: 'introduction' },
                        { text: '安装', link: 'install' },
                        { text: '快速开始', link: 'getting-started' },
                        { text: '开发者', link: 'developer' }
                    ]
                },
                {
                    text: '例子',
                    collapsed: false,
                    items: [
                        { text: 'OBS Studio', link: 'obs-studio' },
                        { text: 'Gstreamer', link: 'gstreamer' },
                    ]
                },
                {
                    text: '参考',
                    collapsed: false,
                    items: [
                        { text: 'Live777 API', link: 'live777-api' },
                        { text: 'LiveMan API', link: 'liveman-api' },
                    ]
                }
            ]},
        },
    }
})
