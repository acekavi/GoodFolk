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
	const script = paintFirst('Promise.all([]).then(([kit, app]) => { start(); });');
	const done: Promise<void> = new Function(
		'requestAnimationFrame',
		'PerformanceObserver',
		'setTimeout',
		'start',
		`return ${script}`
	)(
		(frame: () => void) => {
			started.push('frame');
			frame();
		},
		Observer,
		(callback: () => void, ms?: number) => (ms ? timers.push(callback) : callback()),
		() => started.push('app')
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

	it('matches the boot of the installed SvelteKit', () => {
		const kit = dirname(createRequire(import.meta.url).resolve('@sveltejs/kit/package.json'));
		const render = readFileSync(join(kit, 'src/runtime/server/page/render.js'), 'utf8');

		expect(render).toContain('? `Promise.all([');
		expect(render).toContain(']).then(([kit, app]) => {');
	});
});
