import { defineConfig } from 'vitepress'

export const en = defineConfig({
    lang: 'en-US',
    description: "A very simple, high performance, edge WebRTC SFU",

    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Guide', link: '/guide/what-is-live777' }
        ],

        sidebar: {
            '/guide/': { base: '/guide/', items: [
                {
                    text: 'Introduction',
                    collapsed: false,
                    items: [
                        { text: 'What is live777 ?', link: 'what-is-live777' },
                        { text: 'Introduction', link: 'introduction' },
                        { text: 'Install', link: 'install' },
                        { text: 'Getting Started', link: 'getting-started' },
                        { text: 'For developer', link: 'developer' }
                    ]
                },
                {
                    text: 'Example',
                    collapsed: false,
                    items: [
                        { text: 'OBS Studio', link: 'obs-studio' },
                        { text: 'Gstreamer', link: 'gstreamer' },
                    ]
                },
                {
                    text: 'Reference',
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
