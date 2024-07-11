import { defineConfig } from 'vitepress'

export const en = defineConfig({
    lang: 'en-US',
    description: "A very simple, high performance, edge WebRTC SFU",

    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Guide', link: '/guide/what-is-live777' }
        ],

        sidebar: [
            {
                text: 'Guide',
                collapsed: true,
                items: [
                    { text: 'What is live777', link: '/guide/what-is-live777' },
                    { text: 'Introduction', link: '/guide/introduction' },
                    { text: 'Getting Started', link: '/guide/getting-started' },
                    { text: 'OBS Studio', link: '/guide/obs-studio' },
                    { text: 'Gstreamer', link: '/guide/gstreamer' },
                    { text: 'For developer', link: '/guide/developer' }
                ]
            },
            {
                text: 'API',
                items: [
                    { text: 'Live777 API', link: '/live777-api' },
                    { text: 'LiveMan API', link: '/liveman-api' },
                ]
            }
        ],
    }
})
