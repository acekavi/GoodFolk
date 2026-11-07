import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { paintFirst } from './hydrate';

/** The boot script as SvelteKit 2 writes it for a split bundle. */
const BOOT = `<script>
	{
		const element = document.currentScript.parentElement;

		Promise.all([
			import("./_app/immutable/entry/start.js"),
			import("./_app/immutable/entry/app.js")
		]).then(([kit, app]) => {
			kit.start(app, element, { node_ids: [0, 2] });
		});
	}
</script>`;

/** Runs a transformed boot with fake browser APIs; `started` records what happened, in order. */
function boot(lcpReported: boolean) {
	const started: string[] = [];
	const report: (() => void)[] = [];
	const timers: (() => void)[] = [];
	class Observer {
		static supportedEntryTypes = lcpReported ? ['largest-contentful-paint'] : [];
		constructor(callback: (list: unknown, seen: { disconnect(): void }) => void) {
			report.push(() => callback(null, { disconnect: () => started.push('disconnected') }));
		}
		observe() {}
	}
	const page = paintFirst(
		'<script>const element = document.currentScript.parentElement;\n' +
			'return Promise.all([]).then(([kit, app]) => { start(); });</script>'
	);
	const document = { currentScript: { parentElement: {} } };
	const done: Promise<void> = new Function(
		'requestAnimationFrame',
		'PerformanceObserver',
		'setTimeout',
		'start',
		'document',
		page.replace(/<\/?script>/g, '')
	)(
		(frame: () => void) => {
			started.push('frame');
			frame();
		},
		Observer,
		(callback: () => void, ms?: number) => (ms ? timers.push(callback) : callback()),
		() => started.push('app'),
		document
	);
	return { started, done, report, timers };
}

describe('paintFirst', () => {
	it('waits before the imports, still reading its element at once', () => {
		const html = paintFirst(BOOT);

		expect(html).toContain('.then(() => Promise.all([');
		expect(html).toContain('])).then(([kit, app]) => {');
		expect(html.indexOf('document.currentScript')).toBeLessThan(
			html.indexOf('new Promise((painted)')
		);
		expect(() => new Function(html.replace(/<\/?script>/g, ''))).not.toThrow();
	});

	it('starts the app once the first contentful frame is reported', async () => {
		const { started, done, report } = boot(true);
		await Promise.resolve();
		expect(started).toEqual([]);

		report[0]();
		await done;

		expect(started).toEqual(['disconnected', 'app']);
	});

	it('starts it after the next frame where the browser reports none', async () => {
		const { started, done } = boot(false);
		await done;

		expect(started).toEqual(['frame', 'app']);
	});

	it('starts it within 3 s when no frame is reported', async () => {
		const { started, done, timers } = boot(true);
		timers[0]();
		await done;

		expect(started).toEqual(['app']);
	});

	it('leaves a page without the boot as it is', () => {
		expect(paintFirst('<p>Promise.all([ in prose</p>')).toBe('<p>Promise.all([ in prose</p>');
	});

	it('touches only the boot, whatever the page content says', () => {
		// A hotel's description holding the markers, as the page body and its serialized data carry it.
		const text = 'We serve Promise.all([ breakfast ]).then(([kit, app]) => { and more';
		const html = `<p>${text}</p><script type="application/json" data-sveltekit-fetched>"${text}"</script>${BOOT}`;

		const out = paintFirst(html);

		expect(out.slice(0, out.indexOf(BOOT.slice(0, 20)))).toBe(
			html.slice(0, html.indexOf(BOOT.slice(0, 20)))
		);
		expect(out).toContain('.then(() => Promise.all([');
		const boot = out.slice(
			out.lastIndexOf('<script>') + '<script>'.length,
			out.lastIndexOf('</script>')
		);
		expect(() => new Function(boot)).not.toThrow();
	});

	it('leaves a boot it does not recognise as it is', () => {
		const other = BOOT.replace(
			']).then(([kit, app]) => {',
			']).then((modules) => { const [kit, app] = modules;'
		);

		expect(paintFirst(other)).toBe(other);
	});

	describe('the installed SvelteKit', () => {
		const kit = dirname(createRequire(import.meta.url).resolve('@sveltejs/kit/package.json'));
		const render = readFileSync(join(kit, 'src/runtime/server/page/render.js'), 'utf8');

		it('writes the markers once each, in order, after reading its element', () => {
			const count = (marker: string) => render.split(marker).length - 1;
			expect(count('? `Promise.all([')).toBe(1);
			expect(count(']).then(([kit, app]) => {')).toBe(1);
			expect(render.indexOf('document.currentScript.parentElement')).toBeLessThan(
				render.indexOf('? `Promise.all([')
			);
			expect(render.indexOf('? `Promise.all([')).toBeLessThan(
				render.indexOf(']).then(([kit, app]) => {')
			);
		});

		it('has a boot that still parses once transformed', () => {
			// The split-bundle boot from kit's own template, its interpolations filled in.
			const from = render.indexOf('`Promise.all([') + 1;
			const template = render.slice(from, render.indexOf('});`', from) + '});'.length);
			const filled = template.replace(/\$\{([^}]*)\}/g, (_, name: string) =>
				name.includes('client.start')
					? '"./start.js"'
					: name.includes('client.app')
						? '"./app.js"'
						: name.includes('args')
							? 'element, { node_ids: [0, 2] }'
							: ''
			);
			const page = `<script>\n{\nconst element = document.currentScript.parentElement;\n${filled}\n}\n</script>`;

			const out = paintFirst(page);

			expect(out).not.toBe(page);
			expect(() => new Function(out.replace(/<\/?script>/g, ''))).not.toThrow();
		});
	});
});
