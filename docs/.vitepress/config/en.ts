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
                text: 'Introduction',
                collapsed: false,
                items: [
                    { text: 'What is live777 ?', link: '/guide/what-is-live777' },
                    { text: 'Introduction', link: '/guide/introduction' },
                    { text: 'Install', link: '/guide/install' },
                    { text: 'Getting Started', link: '/guide/getting-started' },
                    { text: 'For developer', link: '/guide/developer' }
                ]
            },
            {
                text: 'Example',
                collapsed: false,
                items: [
                    { text: 'OBS Studio', link: '/guide/obs-studio' },
                    { text: 'Gstreamer', link: '/guide/gstreamer' },
                ]
            },
            {
                text: 'Reference',
                collapsed: false,
                items: [
                    { text: 'Live777 API', link: '/reference/live777-api' },
                    { text: 'LiveMan API', link: '/reference/liveman-api' },
                ]
            }
        ],
    }
})
