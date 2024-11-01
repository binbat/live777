import js from '@eslint/js';
import ts from 'typescript-eslint';

import stylisticJs from '@stylistic/eslint-plugin-js';

export default ts.config(
    {
        ignores: [
            'assets/',
            'web/shared/tools/debugger/**/*.js'
        ]
    },
    js.configs.recommended,
    {
        // js options
        plugins: {
            '@stylistic/js': stylisticJs
        },
        rules: {
            '@stylistic/js/semi': ['warn', 'always'],
            '@stylistic/js/quotes': ['error', 'single', { 'avoidEscape': true }],
            '@stylistic/js/indent': ['warn', 4, { 'SwitchCase': 1 }],
            '@stylistic/js/jsx-quotes': ['error', 'prefer-double']
        }
    },
    ...ts.configs.recommended,
    {
        // ts options
        rules: {
            '@typescript-eslint/no-unused-vars': [
                'error',
                {
                    'args': 'all',
                    'argsIgnorePattern': '^_',
                    'caughtErrors': 'all',
                    'caughtErrorsIgnorePattern': '^_',
                    'destructuredArrayIgnorePattern': '^_',
                    'varsIgnorePattern': '^_',
                    'ignoreRestSiblings': true
                }
            ]
        }
    }
);
