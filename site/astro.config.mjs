// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

const repo = 'https://github.com/samishal1998/bifrost';

export default defineConfig({
	site: 'https://samishal1998.github.io',
	base: '/bifrost',
	integrations: [
		starlight({
			title: 'Bifröst',
			description:
				'Remote worlds. Local files. A daemon, CLI and TUI that keep remote machines mounted as local folders.',
			logo: {
				light: './src/assets/logo-ink-dark.png',
				dark: './src/assets/logo-ink-light.png',
				replacesTitle: true,
			},
			favicon: '/favicon.png',
			social: [{ icon: 'github', label: 'GitHub', href: repo }],
			editLink: { baseUrl: `${repo}/edit/main/site/` },
			customCss: ['./src/styles/custom.css'],
			components: {
				Hero: './src/components/Hero.astro',
				SiteTitle: './src/components/SiteTitle.astro',
			},
			sidebar: [
				{ label: 'Getting started', items: ['installation', 'quickstart'] },
				{
					label: 'Guides',
					items: [
						'guides/configuration',
						'guides/policy',
						'guides/discovery',
						'guides/mount-drivers',
						'guides/service',
						'guides/troubleshooting',
					],
				},
				{ label: 'Examples', items: ['examples/discovery', 'examples/dns-records'] },
				{
					label: 'Reference',
					items: [
						'reference/cli',
						'reference/tui',
						'reference/configuration',
						'reference/api',
						'reference/files',
					],
				},
				{
					label: 'Concepts',
					items: ['concepts/architecture', 'concepts/reconciliation', 'concepts/security'],
				},
				'contributing',
			],
		}),
	],
});
