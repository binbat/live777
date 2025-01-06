import { type LocaleConfig } from './types';

export const en: LocaleConfig = {
    lang: 'en-US',
    label: 'English',
    description: 'A very simple, high performance, edge WebRTC SFU',

    themeConfig: {
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Guide', link: '/guide/what-is-live777' }
        ],

        sidebar: {
            '/guide/': {
                base: '/guide/',
                items: [
                    {
                        text: 'Introduction',
                        collapsed: false,
                        items: [
                            { text: 'What is live777 ?', link: 'what-is-live777' },
                            { text: 'Installation', link: 'installation' },
                            { text: 'Getting Started', link: 'getting-started' },
                            { text: 'For developer', link: 'developer' },
                        ]
                    },
                    {
                        text: 'Components',
                        collapsed: false,
                        items: [
                            { text: 'Live777', link: 'live777' },
                            { text: 'Web UI', link: 'webui' },
                            { text: 'LiveMan', link: 'liveman' },
                            { text: 'WhipInto', link: 'whipinto' },
                            { text: 'WhepFrom', link: 'whepfrom' },
                            { text: 'NET4MQTT', link: 'net4mqtt' },
                        ]
                    },
                    {
                        text: 'Example',
                        collapsed: false,
                        items: [
                            { text: 'OBS Studio', link: 'obs-studio' },
                            { text: 'FFmpeg', link: 'ffmpeg' },
                            { text: 'Gstreamer', link: 'gstreamer' },
                            { text: 'VLC', link: 'vlc' },
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
                ]
            },
        },
    }
};
