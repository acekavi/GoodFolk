import { expect, test, type APIRequestContext } from '@playwright/test';
import { createProperty, signUp } from './helpers';

// Phase 1 gate: the inventory month grid of a 200-room, 12-type property renders in under 50 ms and
// scrolls at 60 fps, with only the columns in view in the DOM. Timings on shared CI runners are noise,
// so this test is left out of the default run. Run it locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf

const ROOM_TYPES = 12;
const ROOMS = 200;

async function post(api: APIRequestContext, path: string, data: object) {
	const response = await api.post(path, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data
	});
	expect(response.status(), await response.text()).toBe(201);
	return response.json();
}

test('the month grid renders under 50 ms and scrolls at 60 fps @perf', async ({ page }) => {
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
	expect(renderMs).toBeLessThan(50);
	expect(slow).toBeLessThanOrEqual(3);
	// Only the columns in view, two of overscan on each side and the active one are in the DOM.
	expect(cells).toBeLessThanOrEqual(ROOM_TYPES * (columnsInView + 2 * 2 + 1));
});
