// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// GitHub Pages: https://lesim-co-ltd.github.io/konoma/
export default defineConfig({
	site: 'https://lesim-co-ltd.github.io',
	base: '/konoma/',
	integrations: [
		starlight({
			title: 'konoma',
			description:
				'A full-screen preview-focused terminal file browser for macOS, built for working next to an AI coding agent.',
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/LESIM-Co-Ltd/konoma' },
			],
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ja: { label: '日本語', lang: 'ja' },
			},
			sidebar: [
				{
					label: 'Start here',
					translations: { ja: 'はじめに' },
					items: [{ slug: 'getting-started' }],
				},
				{
					label: 'Guides',
					translations: { ja: 'ガイド' },
					items: [
						{ slug: 'guides/agent-watch' },
						{ slug: 'guides/preview' },
						{ slug: 'guides/git' },
						{ slug: 'guides/files' },
					],
				},
				{
					label: 'Reference',
					translations: { ja: 'リファレンス' },
					items: [{ slug: 'reference/configuration' }, { slug: 'reference/keymap' }],
				},
			],
		}),
	],
});
