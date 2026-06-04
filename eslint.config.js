import js from '@eslint/js';
import ts from 'typescript-eslint';

export default ts.config(
    {
        ignores: [
            'assets/',
            'docs/.vitepress/cache/',
            'docs/.vitepress/dist/',
            'node_modules/',
            'target/',
            'web/**/dist/',
            'web/shared/tools/debugger/**/*.js'
        ]
    },
    js.configs.recommended,
    ...ts.configs.recommended,
    {
        // ts options
        rules: {
            'no-undef': 'off',
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
