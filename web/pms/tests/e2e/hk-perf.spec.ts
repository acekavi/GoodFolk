import { expect, test, type Browser, type BrowserContext, type Page } from '@playwright/test';
import { addStaff, bookableHotel, createProperty, signIn, signUp } from './helpers';

// Phase 5 gates for the housekeeping screens, on a 500-room property: the board renders under 50 ms (p90 of
// ten loads with its data cached, the API warm), and the housekeeper app's first load on throttled 3G
// (1.6 Mbps down, 750 kbps up, 150 ms latency) is under 2 s (p90 of five cold contexts). A reload offline
// after install showing My rooms is asserted, not timed. Timings on shared CI runners are noise, so the timed
// ones are left out of the default run. Run them locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf
// The numbers depend on the CPU governor (`powersave` on the development laptop) and on other heavy jobs, so
// record both with a result. Every gate is read inside the page with `performance.now()`, as in tape-perf.spec.ts.

// One test at a time, on one seeded property: seeding beside a timing skews it.
test.describe.configure({ mode: 'serial' });
// The trace's DOM snapshots run in the page beside the action being timed and add tens of milliseconds to it.
test.use({ trace: 'off' });

const ROOMS = 500;
/** The housekeeper's own rooms: a day's list, not the whole house. */
const ASSIGNED = 20;

let owner: Page;
let propertyId: string;
let housekeeper: string;

test.beforeAll(async ({ browser }) => {
	test.setTimeout(300_000);
	owner = await browser.newPage();
	const { email } = await signUp(owner);
	await createProperty(owner, 'HKP');
	await bookableHotel(owner, ROOMS, 2);
	propertyId = owner.url().split('/p/')[1];
	// Rooms added through the API get their condition row, so the board shows every one of them.
	({ email: housekeeper } = addStaff(email, 'housekeeping', 'Kumari Silva'));
	const graphql = await owner.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ rooms(propertyId: "${propertyId}") { id number } housekeepers(propertyId: "${propertyId}") { userId } }`
		}
	});
	const { rooms, housekeepers } = (await graphql.json()).data as {
		rooms: { id: string; number: string }[];
		housekeepers: { userId: string }[];
	};
	expect(rooms).toHaveLength(ROOMS);
	const path = `/api/v1/properties/${propertyId}`;
	const dirtied = await owner.request.post(`${path}/rooms/conditions`, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data: { room_ids: rooms.map((room) => room.id), condition: 'dirty' }
	});
	expect(dirtied.status(), await dirtied.text()).toBe(200);
	const assigned = await owner.request.put(`${path}/hk-assignments`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			assignments: rooms
				.slice(0, ASSIGNED)
				.map((room) => ({ room_id: room.id, user_id: housekeepers[0].userId }))
		}
	});
	expect(assigned.status(), await assigned.text()).toBe(204);
});

test.afterAll(async () => {
	await owner.close();
});

const sorted = (values: number[]) => [...values].sort((a, b) => a - b);
const percentile = (values: number[], p: number) =>
	sorted(values)[Math.ceil(p * values.length) - 1];
const median = (values: number[]) => sorted(values)[Math.floor(values.length / 2)];
const summary = (values: number[]) =>
	`${values.map((ms) => ms.toFixed(0)).join(', ')} ms, median ${median(values).toFixed(0)}, ` +
	`p90 ${percentile(values, 0.9).toFixed(0)}, max ${Math.max(...values).toFixed(0)}`;

/** The tile buttons or images of the board, which draws only the rows in view. */
const TILES = '[role="grid"][aria-label="Rooms"] .tile';

/**
 * The page's own clock for one in-app navigation to the board: from the click on the Housekeeping link, made
 * by the page itself so that no test-runner work sits on the main thread beside the render, to the frame after
 * the first row of tiles (and every one after it that the board draws) is in the DOM, then a task after that
 * frame has painted. Resolves with the milliseconds.
 */
function timeBoard(page: Page) {
	return page.evaluate(
		(tiles) =>
			new Promise<number>((resolve) => {
				const link = [...document.querySelectorAll('nav a')].find(
					(a) => a.textContent?.trim() === 'Housekeeping'
				) as HTMLAnchorElement;
				// Counting tiles reads no layout, which would otherwise add to the time on every mutation.
				const painted = () => {
					const grid = document.querySelector('[role="grid"][aria-label="Rooms"]');
					const columns = Number(grid?.getAttribute('aria-colcount'));
					return columns > 0 && document.querySelectorAll(tiles).length >= columns;
				};
				const t0 = performance.now();
				const observer = new MutationObserver(() => {
					if (!painted()) return;
					observer.disconnect();
					requestAnimationFrame(() => {
						const channel = new MessageChannel();
						channel.port1.onmessage = () => resolve(performance.now() - t0);
						channel.port2.postMessage(null);
					});
				});
				observer.observe(document.body, { subtree: true, childList: true, attributes: true });
				link.click();
			}),
		TILES
	);
}

test('the board for 500 rooms renders under 50 ms at p90 over ten loads @perf', async () => {
	await owner.goto(`/p/${propertyId}/housekeeping`);
	await expect(owner.locator(TILES).first()).toBeVisible();
	const overview = owner.getByRole('link', { name: 'Overview', exact: true });
	const timings: number[] = [];
	for (let load = 0; load < 10; load++) {
		// Leave the board and come back: the lists are cached, the API is warm, and what is timed is the
		// board drawing 500 tiles, not the network.
		await overview.click();
		await expect(owner.getByRole('heading', { name: 'Hotel HKP' })).toBeVisible();
		await owner.waitForTimeout(200);
		timings.push(await timeBoard(owner));
		await owner.waitForLoadState('networkidle');
	}
	// Something really rendered: the grid holds all 500 rooms, and the tiles in view show their condition.
	const grid = owner.getByRole('grid', { name: 'Rooms' });
	const rows = Number(await grid.getAttribute('aria-rowcount'));
	const columns = Number(await grid.getAttribute('aria-colcount'));
	expect(rows * columns).toBeGreaterThanOrEqual(ROOMS);
	expect(rows * columns).toBeLessThan(ROOMS + columns);
	const mounted = await owner.locator(TILES).count();
	expect(mounted).toBeGreaterThanOrEqual(columns);
	expect(mounted).toBeLessThan(ROOMS);
	// The tiles fill the board's visible area: the last one drawn reaches the bottom of what is in view.
	const view = (await grid.boundingBox())!;
	const last = (await owner.locator(TILES).last().boundingBox())!;
	expect(last.y + last.height).toBeGreaterThanOrEqual(
		Math.min(view.y + view.height, owner.viewportSize()!.height)
	);
	await expect(owner.locator(`${TILES}[data-condition="DIRTY"]`)).toHaveCount(mounted);
	console.log(`hk board: ${summary(timings)}`);
	expect(percentile(timings, 0.9)).toBeLessThan(50);
});

/** A context signed in as the housekeeper, whose session a cold context can then start from. */
async function housekeeperSession(browser: Browser) {
	const context = await browser.newContext({ viewport: { width: 375, height: 700 } });
	const page = await context.newPage();
	await signIn(page, housekeeper);
	const session = await context.storageState();
	await context.close();
	return session;
}

/** The first load of `/hk` in a new context on 3G: milliseconds from navigation to the list painted. */
async function coldLoad(
	browser: Browser,
	session: Awaited<ReturnType<typeof housekeeperSession>>,
	serviceWorkers: 'allow' | 'block'
) {
	const context = await browser.newContext({
		storageState: session,
		viewport: { width: 375, height: 700 },
		serviceWorkers
	});
	const page = await context.newPage();
	await throttle3G(page);
	await page.addInitScript((assigned) => {
		(window as unknown as { firstPaint: Promise<number> }).firstPaint = new Promise((resolve) => {
			const observer = new MutationObserver(() => {
				const rooms = document.querySelectorAll('ul[aria-label="Rooms"] > li').length;
				if (rooms !== assigned) return;
				observer.disconnect();
				requestAnimationFrame(() => {
					const channel = new MessageChannel();
					channel.port1.onmessage = () => resolve(performance.now());
					channel.port2.postMessage(null);
				});
			});
			observer.observe(document, { subtree: true, childList: true, attributes: true });
		});
	}, ASSIGNED);
	await page.goto('/hk');
	const ms = await page.evaluate(
		() => (window as never as { firstPaint: Promise<number> }).firstPaint
	);
	// Something really rendered: the housekeeper's own rooms, in the housekeeper's own app.
	await expect(page.getByRole('heading', { name: 'My rooms' })).toBeVisible();
	await expect(page.getByRole('list', { name: 'Rooms' }).getByRole('listitem')).toHaveCount(
		ASSIGNED
	);
	await context.close();
	return ms;
}

/** Throttles the page's network to the spec's 3G class, through the DevTools protocol. */
async function throttle3G(page: Page) {
	const client = await page.context().newCDPSession(page);
	await client.send('Network.enable');
	await client.send('Network.emulateNetworkConditions', {
		offline: false,
		latency: 150,
		downloadThroughput: (1.6 * 1_000_000) / 8,
		uploadThroughput: (750 * 1_000) / 8
	});
}

test('the /hk first load on 3G paints My rooms under 2 s at p90 over five cold contexts @perf', async ({
	browser
}) => {
	test.setTimeout(180_000);
	const session = await housekeeperSession(browser);
	const withoutWorker: number[] = [];
	const timings: number[] = [];
	// The worker's install precaches the whole build beside the page's own requests; measured with and
	// without it, interleaved so drift in the machine falls on both alike.
	for (let run = 0; run < 5; run++) {
		withoutWorker.push(await coldLoad(browser, session, 'block'));
		timings.push(await coldLoad(browser, session, 'allow'));
	}
	console.log(`hk 3G first load, service worker blocked: ${summary(withoutWorker)}`);
	console.log(`hk 3G first load: ${summary(timings)}`);
	expect(percentile(timings, 0.9)).toBeLessThan(2000);
});

/** Waits until the page's service worker is active, so the app shell is cached and works offline. */
async function installed(page: Page) {
	await page.evaluate(() => navigator.serviceWorker.ready.then(() => undefined));
}

test('after install, a reload offline shows My rooms', async ({ browser }) => {
	const context: BrowserContext = await browser.newContext({
		viewport: { width: 375, height: 700 }
	});
	const page = await context.newPage();
	await signIn(page, housekeeper);
	await page.goto(`/hk?property=${propertyId}`);
	const rooms = page.getByRole('list', { name: 'Rooms' }).getByRole('listitem');
	await expect(rooms).toHaveCount(ASSIGNED);
	await installed(page);
	// Let the saved list reach IndexedDB before the connection goes.
	await expect
		.poll(() =>
			page.evaluate(
				() =>
					new Promise<boolean>((done) => {
						const open = indexedDB.open('goodfolk-hk');
						open.onsuccess = () => {
							const read = open.result.transaction('kv').objectStore('kv').getAllKeys();
							read.onsuccess = () =>
								done(read.result.some((key) => String(key).startsWith('last:')));
							read.onerror = () => done(false);
						};
						open.onerror = () => done(false);
					})
			)
		)
		.toBe(true);

	await context.setOffline(true);
	await page.reload();
	await expect(page.getByRole('heading', { name: 'My rooms' })).toBeVisible();
	await expect(rooms).toHaveCount(ASSIGNED);
	await expect(page.getByText('You are offline.')).toBeVisible();
	await context.close();
});
