import { expect, test } from '@playwright/test';
import { book, bookableHotel, createProperty, post, signUp } from './helpers';

// Phase 1 gate: the inventory month grid of a 200-room, 12-type property renders in under 60 ms and
// scrolls at 60 fps, with only the columns in view in the DOM. Timings on shared CI runners are noise,
// so this test is left out of the default run. Run it locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf
//
// On a laptop with the `powersave` CPU governor this measures 41-58 ms: with the event stream connected once
// (Phase 3a), nothing refetches inside the timed switches, and the old 37-41 ms reading depended on the
// reconnect-loop bug keeping the CPU clocked up (see ROADMAP Phase 3b). The owner raised the gate from 50 ms to
// 55 ms for this, then to 60 ms when single runs still read 55.8-55.9 ms; measure with the `performance`
// governor or on the server class when it matters.

// One test at a time: seeding one test's data beside another's timing skews it.
test.describe.configure({ mode: 'default' });

const ROOM_TYPES = 12;
const ROOMS = 200;

test('the month grid renders under 60 ms and scrolls at 60 fps @perf', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'BIG');
	const property = `/api/v1/properties/${page.url().split('/p/')[1]}`;
	for (let index = 0; index < ROOM_TYPES; index++) {
		const type = await post(page.request, `${property}/room-types`, {
			code: `T${index}`,
			name: `Type ${index}`,
			base_occupancy: 2,
			max_adults: 2,
			max_children: 1,
			max_occupancy: 3
		});
		const count = Math.floor(ROOMS / ROOM_TYPES) + (index < ROOMS % ROOM_TYPES ? 1 : 0);
		const first = (index + 1) * 100 + 1;
		await post(page.request, `${property}/rooms/bulk`, {
			room_type_id: type.id,
			first,
			last: first + count - 1
		});
	}
	await page.getByRole('link', { name: 'Inventory' }).click();
	const grid = page.getByRole('grid', { name: 'Availability' });
	await expect(grid.getByRole('row')).toHaveCount(ROOM_TYPES + 1);
	// Load next month once and come back, so the timing below measures rendering, not the network.
	await page.getByRole('button', { name: 'Next month' }).click();
	await expect(grid.getByRole('gridcell', { name: /available/ }).first()).toBeVisible();
	await page.getByRole('button', { name: 'Previous month' }).click();
	await expect(grid.getByRole('gridcell', { name: /available/ }).first()).toBeVisible();

	// Median of ten switches between the two loaded months.
	const renderMs = await page.evaluate(async () => {
		const timings: number[] = [];
		for (let switches = 0; switches < 10; switches++) {
			const label = switches % 2 === 0 ? 'Next month' : 'Previous month';
			const button = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
			const start = performance.now();
			button.click();
			// Svelte applies the update in a microtask; reading the grid's size forces style and layout.
			await new Promise((resolve) => setTimeout(resolve, 0));
			document.querySelector('[role="grid"]')!.getBoundingClientRect();
			timings.push(performance.now() - start);
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
		timings.sort((a, b) => a - b);
		return timings[5];
	});
	const frames = await page.evaluate(async () => {
		const viewport = document.querySelector<HTMLElement>('[role="grid"]')!;
		const gaps: number[] = [];
		let last = performance.now();
		for (let frame = 0; frame < 90; frame++) {
			viewport.scrollLeft += 16;
			await new Promise((resolve) => requestAnimationFrame(resolve));
			const now = performance.now();
			gaps.push(now - last);
			last = now;
		}
		return gaps;
	});
	const cells = await grid.getByRole('gridcell').count();
	// Columns that fit beside the 160 px row labels at 64 px each, plus a partial one.
	const columnsInView = await page.evaluate(
		() => Math.ceil((document.querySelector('[role="grid"]')!.clientWidth - 160) / 64) + 1
	);

	const slow = frames.filter((gap) => gap > 25).length;
	console.log(
		`month grid: render ${renderMs.toFixed(1)} ms, ${slow}/90 slow frames, ${cells} cells`
	);
	expect(renderMs).toBeLessThan(60);
	expect(slow).toBeLessThanOrEqual(3);
	// Only the columns in view, two of overscan on each side and the active one are in the DOM.
	expect(cells).toBeLessThanOrEqual(ROOM_TYPES * (columnsInView + 2 * 2 + 1));
});

// Phase 3 gate: scrolling 10k reservation rooms stays at 60 fps with a fixed DOM row count.
const NIGHTS = 50;
const HOTEL_ROOMS = 200;
const ROOMS_PER_BOOKING = 10;

test('the reservations table scrolls 10k rows at 60 fps with a fixed DOM row count @perf', async ({
	page
}) => {
	test.setTimeout(900_000);
	await signUp(page);
	await createProperty(page, 'BIG');
	// 200 rooms, every one booked on each of 50 nights: 1,000 reservations of ten one-night rooms.
	const hotel = await bookableHotel(page, HOTEL_ROOMS, NIGHTS);
	const bookings = Array.from({ length: (NIGHTS * HOTEL_ROOMS) / ROOMS_PER_BOOKING }, (_, index) =>
		Math.floor((index * ROOMS_PER_BOOKING) / HOTEL_ROOMS)
	);
	for (let start = 0; start < bookings.length; start += 8) {
		await Promise.all(
			bookings
				.slice(start, start + 8)
				.map((night) => book(page.request, hotel, Array(ROOMS_PER_BOOKING).fill(night)))
		);
	}
	const total = NIGHTS * HOTEL_ROOMS;

	await page.getByRole('link', { name: 'Reservations' }).click();
	const table = page.getByRole('table', { name: 'Reservations' });
	await expect(table).toHaveAttribute('aria-rowcount', String(total + 1));
	// Load every page first, so the timing below measures scrolling, not the network.
	await expect
		.poll(
			() =>
				table.evaluate((scroller) => {
					scroller.scrollTop = scroller.scrollHeight;
					return scroller.scrollHeight;
				}),
			{ timeout: 300_000, intervals: [50] }
		)
		.toBeGreaterThanOrEqual(total * 36);
	await table.evaluate((scroller) => (scroller.scrollTop = 0));

	// 90 frames, five rows a frame, with the DOM row count sampled on every frame.
	const { gaps, counts } = await table.evaluate(async (scroller) => {
		const gaps: number[] = [];
		const counts: number[] = [];
		let last = performance.now();
		for (let frame = 0; frame < 90; frame++) {
			scroller.scrollTop += 5 * 36;
			await new Promise((resolve) => requestAnimationFrame(resolve));
			const now = performance.now();
			gaps.push(now - last);
			last = now;
			counts.push(scroller.querySelectorAll('[role="row"]').length);
		}
		return { gaps, counts };
	});
	// Rows in view plus a partial one, five of overscan each side, the header and the focused row.
	const bound = await table.evaluate((scroller) => Math.ceil(scroller.clientHeight / 36) + 1 + 12);

	const slow = gaps.filter((gap) => gap > 25).length;
	const sorted = [...gaps].sort((a, b) => a - b);
	console.log(
		`reservations table: ${slow}/90 slow frames, median frame ${sorted[45].toFixed(1)} ms, ` +
			`DOM rows ${Math.min(...counts)}–${Math.max(...counts)} (bound ${bound})`
	);
	expect(slow).toBeLessThanOrEqual(3);
	expect(Math.max(...counts)).toBeLessThanOrEqual(bound);
});
