import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';

const ROUTES = join(import.meta.dirname, '..', 'routes');

/** Every page component under `src/routes`, with its source. */
function pages(dir = ROUTES): { path: string; source: string }[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) return pages(path);
		return entry.name.endsWith('.svelte')
			? [{ path: relative(ROUTES, path), source: readFileSync(path, 'utf8') }]
			: [];
	});
}

/** The `<a …>` opening tags of `source`. */
function anchors(source: string): string[] {
	return source.match(/<a\b[^>]*>/g) ?? [];
}

describe('links into the manage page', () => {
	it('load it as a new document, so no script of the book page (Turnstile) runs beside the token', () => {
		const intoManage = pages().flatMap(({ path, source }) =>
			anchors(source)
				.filter((tag) => /manage/i.test(tag.match(/href=\{?"?([^\s>]*)/)?.[1] ?? ''))
				.map((tag) => ({ path, tag }))
		);

		expect(intoManage.map(({ path }) => path)).toContain(
			'[code=code]/booked/[confirmation]/+page.svelte'
		);
		for (const { tag } of intoManage) {
			expect(tag).toContain('data-sveltekit-reload');
			expect(tag).toContain('data-sveltekit-preload-data="off"');
		}
	});

	it('come from the only page that loads Turnstile’s script: the book page', () => {
		const loading = pages().filter(({ source }) => source.includes('challenges.cloudflare.com'));

		expect(loading.map(({ path }) => path)).toEqual(['[code=code]/book/+page.svelte']);
	});
});
