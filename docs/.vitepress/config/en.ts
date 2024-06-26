import { defineConfig } from 'vitepress'

export const en = defineConfig({
    lang: 'en-US',
    description: "A very simple, high performance, edge WebRTC SFU",

    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Examples', link: '/markdown-examples' }
        ],

        sidebar: [
            {
                text: 'Guide',
                items: [
                    { text: 'What is live777', link: '/guide/what-is-live777' },
                    { text: 'Getting Started', link: '/guide/getting-started' }
                ]
            },
            {
                text: 'API',
                items: [
                    { text: 'Live777 API', link: '/live777-api' }
                ]
            }
        ],
    }
})
