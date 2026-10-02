import { expect, test, type Page } from '@playwright/test';
import { addDays, bookableHotel, createProperty, post, signUp, type Hotel } from './helpers';

// Phase 4 gates for the tape chart, on a 500-room property with 18 months of short stays: first open
// under 400 ms (p90 of ten cold loads, like the page and picker changes), a page or picker change under
// 50 ms, scrolling at 58 fps or better with no long task over 50 ms, the drag ghost following within one
// frame, and fewer than 3,000 DOM nodes. Timings on shared CI
// runners are noise, so these are left out of the default run. Run them locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf
// The numbers depend on the CPU governor (`powersave` on the development laptop) and on other heavy jobs, so
// record both with a result. Every gate is read inside the page with `performance.now()`, as in perf.spec.ts.

// One test at a time, on one seeded property: seeding beside a timing skews it.
test.describe.configure({ mode: 'serial' });

const ROOMS = 500;
const MONTHS = 18;
const NIGHTS = Math.round(MONTHS * 30.4);
/** Rooms 101 to 130, the first three pages, are the ones measured; about 80% of them are booked each night. */
const MEASURED_ROOMS = 30;
const OCCUPANCY = 0.8;

/** Repeatable pseudo-random numbers in [0, 1), so every run seeds the same hotel. */
function random(seed: number) {
	return () => {
		seed = (seed + 0x6d2b79f5) | 0;
		let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

/**
 * Stays of one to three nights that keep about `OCCUPANCY` of the measured rooms booked on each night, the
 * count wandering from night to night, in order of check-in. Auto-assignment gives each the lowest free room.
 */
function seedStays(): { night: number; nights: number }[] {
	const next = random(4);
	const stays: { night: number; nights: number }[] = [];
	const ends: number[] = [];
	const mean = MEASURED_ROOMS * OCCUPANCY;
	for (let night = 0; night < NIGHTS; night++) {
		const target = Math.min(MEASURED_ROOMS, Math.round(mean - 6 + next() * 12));
		while (ends.length && ends[0] <= night) ends.shift();
		while (ends.length < target) {
			const nights = Math.min(1 + Math.floor(next() * 3), NIGHTS - night);
			stays.push({ night, nights });
			ends.push(night + nights);
			ends.sort((a, b) => a - b);
		}
	}
	return stays;
}

let page: Page;
let hotel: Hotel;

test.beforeAll(async ({ browser }) => {
	test.setTimeout(900_000);
	page = await browser.newPage();
	await signUp(page);
	await createProperty(page, 'TAP');
	hotel = await bookableHotel(page, ROOMS, NIGHTS);
	// A reservation holds at most ten rooms, each with its own dates.
	const stays = seedStays();
	for (let start = 0; start < stays.length; start += 80) {
		await Promise.all(
			Array.from({ length: 8 }, (_, group) =>
				stays.slice(start + group * 10, start + group * 10 + 10)
			)
				.filter((rooms) => rooms.length > 0)
				.map((rooms) =>
					post(page.request, `${hotel.path}/reservations`, {
						booker_guest_id: hotel.guestId,
						source: 'front_desk',
						rooms: rooms.map(({ night, nights }) => ({
							room_type_id: hotel.roomTypeId,
							rate_plan_id: hotel.ratePlanId,
							meal_plan: 'RO',
							check_in: addDays(hotel.businessDate, night),
							check_out: addDays(hotel.businessDate, night + nights),
							adults: 2
						}))
					})
				)
		);
	}
});

test.afterAll(async () => {
	await page.close();
});

const tapeUrl = () => `${hotel.path.replace('/api/v1/properties/', '/p/')}/tape`;
const sorted = (values: number[]) => [...values].sort((a, b) => a - b);
const percentile = (values: number[], p: number) =>
	sorted(values)[Math.ceil(p * values.length) - 1];
const daysBetween = (from: string, to: string) =>
	Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 86_400_000);
const median = (values: number[]) =>
	[...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

/**
 * The page's own clock for one interaction: from the click (or the Enter key) that starts it to the frame
 * after the chart shows `first` to `last` on its rail, with a bar and only bars of those rooms. The promise
 * resolves with the milliseconds; arm it before the action and await it after.
 */
function timeChange(start: 'click' | 'enter', first: number, last: number) {
	return page.evaluate(
		({ start, first, last }) =>
			new Promise<number>((resolve) => {
				let t0 = -1;
				const painted = () => {
					const rail = document.querySelector('ol[aria-label="Rooms"] li');
					if (!rail?.textContent?.startsWith(String(first))) return false;
					const bars = [...document.querySelectorAll<HTMLElement>('.bar[data-room]')];
					return (
						bars.length > 0 &&
						bars.every(
							(bar) => Number(bar.dataset.room) >= first && Number(bar.dataset.room) <= last
						)
					);
				};
				const observer = new MutationObserver(() => {
					if (t0 < 0 || !painted()) return;
					observer.disconnect();
					// The frame after the change, then a task after that frame has painted.
					requestAnimationFrame(() => {
						const channel = new MessageChannel();
						channel.port1.onmessage = () => resolve(performance.now() - t0);
						channel.port2.postMessage(null);
					});
				});
				observer.observe(document.body, {
					subtree: true,
					childList: true,
					attributes: true,
					characterData: true
				});
				window.addEventListener(
					start === 'click' ? 'click' : 'keydown',
					(event) => {
						if (start === 'enter' && (event as KeyboardEvent).key !== 'Enter') return;
						t0 = performance.now();
					},
					{ capture: true }
				);
			}),
		{ start, first, last }
	);
}

test('first open of the chart paints its rooms and bars under 400 ms at p90 over ten cold loads @perf', async ({
	browser
}) => {
	const session = await page.context().storageState();
	const timings: number[] = [];
	for (let run = 0; run < 10; run++) {
		// A new context each time: nothing cached, as on a first visit of the day.
		const context = await browser.newContext({ storageState: session });
		const fresh = await context.newPage();
		await fresh.addInitScript(() => {
			(window as unknown as { firstPaint: Promise<number> }).firstPaint = new Promise((resolve) => {
				const observer = new MutationObserver(() => {
					const rooms = document.querySelectorAll('ol[aria-label="Rooms"] li').length;
					if (rooms !== 10 || !document.querySelector('.bar[data-room]')) return;
					observer.disconnect();
					requestAnimationFrame(() => {
						const channel = new MessageChannel();
						channel.port1.onmessage = () => resolve(performance.now());
						channel.port2.postMessage(null);
					});
				});
				observer.observe(document, { subtree: true, childList: true, attributes: true });
			});
		});
		await fresh.goto(tapeUrl());
		timings.push(
			await fresh.evaluate(() => (window as never as { firstPaint: Promise<number> }).firstPaint)
		);
		await context.close();
	}
	console.log(
		`tape first open: ${timings.map((ms) => ms.toFixed(0)).join(', ')} ms, median ${median(timings).toFixed(0)}, ` +
			`p90 ${percentile(timings, 0.9).toFixed(0)}, max ${Math.max(...timings).toFixed(0)}`
	);
	expect(percentile(timings, 0.9)).toBeLessThan(400);
});

test('a page or picker change paints under 50 ms with the tiles prefetched @perf', async () => {
	await page.goto(tapeUrl());
	const picker = page.getByRole('combobox', { name: 'Pick rooms' });
	const next = page.getByRole('button', { name: 'Next' });
	const prev = page.getByRole('button', { name: 'Prev' });
	await expect(page.getByText(`Rooms 1–10 of ${ROOMS}`)).toBeVisible();

	// Hover first, as a user would, and let the tiles arrive; then the click is a cache hit.
	const pageTimes: number[] = [];
	for (let turn = 0; turn < 5; turn++) {
		await next.hover();
		await page.waitForLoadState('networkidle');
		await page.waitForTimeout(300);
		const forward = timeChange('click', 111, 120);
		await next.click();
		pageTimes.push(await forward);
		await prev.hover();
		await page.waitForLoadState('networkidle');
		await page.waitForTimeout(300);
		const back = timeChange('click', 101, 110);
		await prev.click();
		pageTimes.push(await back);
	}

	// A chip change: add the range once so its tiles are cached, remove it, then time adding it again.
	const chipTimes: number[] = [];
	for (let change = 0; change < 10; change++) {
		await picker.fill('121-130');
		await picker.press('Enter');
		await expect(page.getByRole('button', { name: 'Remove 121-130' })).toBeVisible();
		await page.waitForLoadState('networkidle');
		await page.waitForTimeout(300);
		await picker.press('Backspace');
		await expect(page.getByRole('button', { name: 'Remove 121-130' })).toBeHidden();
		await page.waitForLoadState('networkidle');
		await page.waitForTimeout(300);
		await picker.fill('121-130');
		const added = timeChange('enter', 121, 130);
		await picker.press('Enter');
		chipTimes.push(await added);
		await picker.press('Backspace');
		await page.waitForLoadState('networkidle');
		await page.waitForTimeout(300);
	}
	const format = (values: number[]) => values.map((ms) => ms.toFixed(1)).join(', ');
	console.log(
		`tape page change: ${format(pageTimes)} ms, median ${median(pageTimes).toFixed(1)}, p90 ${percentile(pageTimes, 0.9).toFixed(1)}, max ${Math.max(...pageTimes).toFixed(1)}; ` +
			`picker change: ${format(chipTimes)} ms, median ${median(chipTimes).toFixed(1)}, p90 ${percentile(chipTimes, 0.9).toFixed(1)}, max ${Math.max(...chipTimes).toFixed(1)}`
	);
	expect(percentile(pageTimes, 0.9)).toBeLessThan(50);
	expect(percentile(chipTimes, 0.9)).toBeLessThan(50);
});

/** Scrolls the chart `days` forward over `ms`, one animation frame at a time; the frame gaps and long tasks. */
function scrollDays(days: number, ms: number) {
	return page.evaluate(
		async ({ days, ms }) => {
			const viewport = document.querySelector<HTMLElement>('[aria-label="Tape chart"]')!;
			const dayWidth = parseFloat(
				getComputedStyle(viewport.firstElementChild!).getPropertyValue('--day')
			);
			const longTasks: number[] = [];
			const observer = new PerformanceObserver((list) => {
				for (const entry of list.getEntries()) longTasks.push(entry.duration);
			});
			observer.observe({ type: 'longtask', buffered: false });
			const gaps: number[] = [];
			let maxNodes = 0;
			const speed = (days * dayWidth) / ms;
			let last = await new Promise<number>((resolve) => requestAnimationFrame(resolve));
			const begin = last;
			while (last - begin < ms) {
				const now = await new Promise<number>((resolve) => requestAnimationFrame(resolve));
				viewport.scrollLeft += speed * (now - last);
				gaps.push(now - last);
				last = now;
				maxNodes = Math.max(maxNodes, document.querySelectorAll('*').length);
			}
			// Long tasks are reported in a later task.
			await new Promise((resolve) => setTimeout(resolve, 200));
			observer.disconnect();
			return { gaps, longTasks, maxNodes, start: viewport.dataset.start };
		},
		{ days, ms }
	);
}

test('scrolling six months in three seconds holds 58 fps with no long task @perf', async () => {
	await page.goto(tapeUrl());
	await expect(page.getByText(`Rooms 1–10 of ${ROOMS}`)).toBeVisible();
	await expect(page.locator('.bar[data-room]').first()).toBeVisible();
	await page.waitForLoadState('networkidle');
	const before = await page.getByRole('group', { name: 'Tape chart' }).getAttribute('data-start');
	const result = await scrollDays(182, 3000);
	await page.waitForTimeout(500);
	const after = await page.getByRole('group', { name: 'Tape chart' }).getAttribute('data-start');
	const fps = 1000 / median(result.gaps);
	const worst = Math.max(...result.gaps);
	console.log(
		`tape scroll: ${before} to ${after}, ${result.gaps.length} frames, median ${fps.toFixed(1)} fps, ` +
			`worst frame ${worst.toFixed(1)} ms, long tasks [${result.longTasks.map((ms) => ms.toFixed(0)).join(', ')}], ` +
			`DOM peak ${result.maxNodes}`
	);
	// The scroll happened: the first day in view moved by the six months asked for, give or take snapping.
	expect(Math.abs(daysBetween(before!, after!) - 182)).toBeLessThanOrEqual(7);
	expect(fps).toBeGreaterThanOrEqual(58);
	expect(result.longTasks.filter((ms) => ms > 50)).toEqual([]);
});

test('the DOM stays under 3,000 nodes after scrolling twelve months @perf', async () => {
	await page.goto(tapeUrl());
	await expect(page.locator('.bar[data-room]').first()).toBeVisible();
	await page.waitForLoadState('networkidle');
	const origin = (await page
		.getByRole('group', { name: 'Tape chart' })
		.getAttribute('data-start'))!;
	const first = await scrollDays(182, 3000);
	const second = await scrollDays(183, 3000);
	await page.waitForTimeout(500);
	const nodes = await page.evaluate(() => document.querySelectorAll('*').length);
	console.log(
		`tape DOM: ${nodes} nodes after twelve months (peak ${Math.max(first.maxNodes, second.maxNodes)}), ` +
			`now at ${second.start}`
	);
	// The scrolls happened: six months, then a year in all, give or take snapping.
	expect(Math.abs(daysBetween(origin, first.start!) - 182)).toBeLessThanOrEqual(7);
	expect(Math.abs(daysBetween(origin, second.start!) - 365)).toBeLessThanOrEqual(14);
	expect(nodes).toBeLessThan(3000);
	expect(Math.max(first.maxNodes, second.maxNodes)).toBeLessThan(3000);
});

test('the drag ghost follows the pointer within one frame @perf', async () => {
	await page.goto(tapeUrl());
	const bar = page.locator('.bar.draggable[data-room="101"]').first();
	await expect(bar).toBeVisible();
	await page.waitForLoadState('networkidle');
	const box = (await bar.boundingBox())!;
	const grabX = box.x + box.width / 2;
	const grabY = box.y + box.height / 2;
	const dayWidth = await page.evaluate(() =>
		parseFloat(
			getComputedStyle(
				document.querySelector('[aria-label="Tape chart"]')!.firstElementChild!
			).getPropertyValue('--day')
		)
	);

	// Each move is recorded at the window (before the bar's own handler) and at the document (after it): the
	// handler's own time, and the ghost's transform once the handler ran. At the next frame the ghost must still
	// hold the transform the latest move left it with: a ghost moved in a later frame (or by a deferred write)
	// shows there as a different transform.
	await page.evaluate(() => {
		const ghost = document.querySelector<HTMLElement>('.ghost')!;
		const moves: { handler: number; lag: number; set: string; framed: string; latest: string }[] =
			[];
		(window as never as { moves: typeof moves }).moves = moves;
		let t0 = 0;
		let latest = '';
		window.addEventListener('pointermove', () => (t0 = performance.now()), { capture: true });
		document.addEventListener('pointermove', (event) => {
			const handler = performance.now() - t0;
			const set = ghost.style.transform;
			latest = set;
			const stamp = event.timeStamp;
			requestAnimationFrame(() => {
				moves.push({
					handler,
					lag: performance.now() - stamp,
					set,
					framed: ghost.style.transform,
					latest
				});
			});
		});
	});
	await page.mouse.move(grabX, grabY);
	await page.mouse.down();
	const steps = 60;
	for (let step = 1; step <= steps; step++) {
		// Sweep across days, with a little vertical wander, 3/8 of a day a move.
		await page.mouse.move(grabX + step * dayWidth * 0.375, grabY + Math.sin(step / 5) * 20);
	}
	await page.waitForTimeout(100);
	await page.keyboard.press('Escape');
	await page.mouse.up();
	const moves = await page.evaluate(
		() =>
			(
				window as never as {
					moves: { handler: number; lag: number; set: string; framed: string; latest: string }[];
				}
			).moves
	);
	const dragging = moves.filter((move) => move.set !== '');
	const handlers = dragging.map((move) => move.handler);
	const lags = dragging.map((move) => move.lag);
	const stale = dragging.filter((move) => move.latest !== move.framed).length;
	const moved = new Set(dragging.map((move) => move.set)).size;
	console.log(
		`tape drag: ${dragging.length} moves, handler median ${median(handlers).toFixed(2)} ms, max ${Math.max(...handlers).toFixed(2)} ms; ` +
			`move to frame median ${median(lags).toFixed(1)} ms, max ${Math.max(...lags).toFixed(1)} ms; ` +
			`${stale} frames behind, ${moved} distinct transforms`
	);
	expect(dragging.length).toBeGreaterThan(steps / 2);
	expect(moved).toBeGreaterThan(5);
	// No frame shows the ghost anywhere but where the latest move put it.
	expect(stale).toBe(0);
	expect(Math.max(...handlers)).toBeLessThan(16);
	expect(median(lags)).toBeLessThan(16);
});
