import type { Config } from 'tailwindcss';

import daisyui from 'daisyui';

export default {
    content: [
        'node_modules/daisyui/dist/**/*.js',
        'node_modules/react-daisyui/dist/**/*.js',
        'web/**/*.{html,tsx}'
    ],
    theme: {
        extend: {},
    },
    plugins: [
        daisyui
    ],
} satisfies Config;
