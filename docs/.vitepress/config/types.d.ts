import { defineConfig } from 'vitepress';

type LocalesType = NonNullable<Parameters<typeof defineConfig>[0]['locales']>;
type LocalesValueType = LocalesType[keyof LocalesType];

export type LocaleConfig = LocalesValueType;
