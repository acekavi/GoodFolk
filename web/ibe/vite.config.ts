import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Server-rendered on Node: every page is HTML from the first byte, for search engines and a fast LCP.
			adapter: adapter(),
			// The site's own CSS is small: inlined, it costs no render-blocking request before the hero paints.
			inlineStyleThreshold: 16384
		})
	],
	// core-api's development IBE_PUBLIC_ORIGIN is http://localhost:5281: its CORS admits bookings from this port.
	server: { port: 5281, strictPort: true },
	preview: { port: 5281, strictPort: true },
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
