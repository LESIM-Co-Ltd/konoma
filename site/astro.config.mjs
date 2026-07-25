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
				'A full-screen preview-focused terminal file browser for macOS and Linux, built for working next to an AI coding agent.',
			// Social preview (Open Graph / Twitter). The image lives in site/public/,
			// so it is served from <site><base>og.jpg.
			head: [
				{
					tag: 'meta',
					attrs: { property: 'og:image', content: 'https://lesim-co-ltd.github.io/konoma/og.jpg' },
				},
				{ tag: 'meta', attrs: { property: 'og:image:width', content: '1200' } },
				{ tag: 'meta', attrs: { property: 'og:image:height', content: '630' } },
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:alt',
						content: 'konoma — sunlit forest seen through the gap between two trees',
					},
				},
				{ tag: 'meta', attrs: { name: 'twitter:card', content: 'summary_large_image' } },
				{
					tag: 'meta',
					attrs: { name: 'twitter:image', content: 'https://lesim-co-ltd.github.io/konoma/og.jpg' },
				},
			],
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/LESIM-Co-Ltd/konoma' },
			],
			customCss: ['./src/styles/custom.css'],
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ja: { label: '日本語', lang: 'ja' },
			},
			sidebar: [
				{
					label: 'Start here',
					translations: { ja: 'はじめに' },
					items: [{ slug: 'getting-started' }, { slug: 'tutorial' }],
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
