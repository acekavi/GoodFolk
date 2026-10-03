import { createServer, type AddressInfo } from 'node:net';
import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import { housekeepingProperty, signIn } from './helpers';

// Phase 5 gates for the housekeeping screens, on a 500-room property: the board renders under 50 ms (p90 of
// ten loads with its data cached, the API warm), and the housekeeper app's first load on throttled 3G
// (1.6 Mbps down, 750 kbps up, 150 ms latency, for the page and for its service worker) is under 2 s (p90 of
// five cold contexts). Timings on shared CI runners are noise, so the timed ones are left
// out of the default run. Run them locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf
// The numbers depend on the CPU governor (`powersave` on the development laptop) and on other heavy jobs, so
// record both with a result. Every gate is read inside the page with `performance.now()`, as in tape-perf.spec.ts.

// The trace's DOM snapshots run in the page beside the action being timed and add tens of milliseconds to it.
test.use({ trace: 'off' });

const ROOMS = 500;
/** The housekeeper's own rooms: a day's list, not the whole house. */
const ASSIGNED = 20;

const sorted = (values: number[]) => [...values].sort((a, b) => a - b);
const percentile = (values: number[], p: number) =>
	sorted(values)[Math.ceil(p * values.length) - 1];
const median = (values: number[]) => sorted(values)[Math.floor(values.length / 2)];
const summary = (values: number[]) =>
	`${values.map((ms) => ms.toFixed(0)).join(', ')} ms, median ${median(values).toFixed(0)}, ` +
	`p90 ${percentile(values, 0.9).toFixed(0)}, max ${Math.max(...values).toFixed(0)}`;

// One test at a time, on one seeded property: seeding beside a timing skews it.
test.describe('speed gates', () => {
	test.describe.configure({ mode: 'serial' });

	let owner: Page;
	let propertyId: string;
	let housekeeper: string;

	test.beforeAll(async ({ browser }) => {
		test.setTimeout(300_000);
		owner = await browser.newPage();
		({ propertyId, housekeeper } = await housekeepingProperty(owner, 'HKP', ROOMS, ASSIGNED));
	});

	test.afterAll(async () => {
		await owner.close();
	});

	/** The tile buttons or images of the board, which draws only the rows in view. */
	const TILES = '[role="grid"][aria-label="Rooms"] .tile';

	/**
	 * The page's own clock for one in-app navigation to the board: from the click on the Housekeeping link, made
	 * by the page itself so that no test-runner work sits on the main thread beside the render, to the frame after
	 * the tiles of every row in view are in the DOM, then a task after that frame has painted. The rows in view
	 * are worked out before the click (the grid is at most 70vh tall), so that no layout is read while timing.
	 * Resolves with the milliseconds.
	 */
	function timeBoard(page: Page) {
		return page.evaluate(
			(tiles) =>
				new Promise<number>((resolve) => {
					const link = [...document.querySelectorAll('nav a')].find(
						(a) => a.textContent?.trim() === 'Housekeeping'
					) as HTMLAnchorElement;
					// A row is 4.3rem and the gap 0.4rem, as in the board.
					const rem = parseFloat(getComputedStyle(document.documentElement).fontSize);
					const rowsInView = Math.ceil((0.7 * window.innerHeight) / (4.7 * rem));
					const painted = () => {
						const grid = document.querySelector('[role="grid"][aria-label="Rooms"]');
						const columns = Number(grid?.getAttribute('aria-colcount'));
						const rooms = Number(grid?.getAttribute('aria-rowcount')) * columns;
						return (
							columns > 0 &&
							document.querySelectorAll(tiles).length >= Math.min(rooms, rowsInView * columns)
						);
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

	/**
	 * The first load of `/hk` in a new context on 3G: milliseconds from navigation to the list painted, and when
	 * the service worker, if allowed, finished installing (its precache), also from navigation.
	 */
	async function coldLoad(
		browser: Browser,
		session: Awaited<ReturnType<typeof housekeeperSession>>,
		serviceWorkers: 'allow' | 'block',
		workers?: Awaited<ReturnType<typeof throttleServiceWorkers>>
	) {
		const context = await browser.newContext({
			storageState: session,
			viewport: { width: 375, height: 700 },
			serviceWorkers
		});
		const page = await context.newPage();
		await throttle3G(page);
		const before = workers?.count() ?? 0;
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
		const installed = workers
			? await page.evaluate(() => navigator.serviceWorker.ready.then(() => performance.now()))
			: undefined;
		const throttled = workers ? workers.count() - before : undefined;
		await context.close();
		return { ms, installed, throttled };
	}

	const THREE_G = {
		offline: false,
		latency: 150,
		downloadThroughput: (1.6 * 1_000_000) / 8,
		uploadThroughput: (750 * 1_000) / 8
	};

	/** Throttles the page's network to the spec's 3G class, through the DevTools protocol. */
	async function throttle3G(page: Page) {
		const client = await page.context().newCDPSession(page);
		await client.send('Network.enable');
		await client.send('Network.emulateNetworkConditions', THREE_G);
	}

	/**
	 * Throttles every service worker the browser on `port` starts to the same 3G class. A worker is a target of
	 * its own: the page's throttle does not reach the fetches of its install. Each new worker is held at start
	 * until its throttle is set. Playwright's own browser session cannot address a child session, so this is
	 * a small client of the browser's debugging port. `count()` is how many workers were throttled so far.
	 */
	async function throttleServiceWorkers(port: number) {
		const { webSocketDebuggerUrl } = (await (
			await fetch(`http://127.0.0.1:${port}/json/version`)
		).json()) as { webSocketDebuggerUrl: string };
		const socket = new WebSocket(webSocketDebuggerUrl);
		await new Promise((resolve, reject) => {
			socket.onopen = resolve;
			socket.onerror = reject;
		});
		let id = 0;
		let throttled = 0;
		const send = (method: string, params: object = {}, sessionId?: string) =>
			socket.send(JSON.stringify({ id: ++id, method, params, sessionId }));
		socket.onmessage = ({ data }) => {
			const message = JSON.parse(String(data));
			if (message.method !== 'Target.attachedToTarget') return;
			if (message.params.targetInfo.type !== 'service_worker') return;
			const session = message.params.sessionId;
			send('Network.enable', {}, session);
			send('Network.emulateNetworkConditions', THREE_G, session);
			throttled++;
			send('Runtime.runIfWaitingForDebugger', {}, session);
		};
		send('Target.setAutoAttach', {
			autoAttach: true,
			waitForDebuggerOnStart: true,
			flatten: true,
			filter: [{ type: 'service_worker' }, { exclude: true }]
		});
		return { count: () => throttled, stop: () => socket.close() };
	}

	/** A browser of its own that can be reached on a debugging port, for `throttleServiceWorkers`. */
	async function debuggableBrowser() {
		const server = createServer();
		await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
		const { port } = server.address() as AddressInfo;
		await new Promise((resolve) => server.close(resolve));
		const browser = await chromium.launch({
			args: [`--remote-debugging-port=${port}`],
			chromiumSandbox: process.env.PLAYWRIGHT_NO_SANDBOX !== '1'
		});
		return { browser, port };
	}

	test('the /hk first load on 3G paints My rooms under 2 s at p90 over five cold contexts @perf', async ({
		browser
	}) => {
		test.setTimeout(240_000);
		const session = await housekeeperSession(browser);
		// A browser of its own, so that the workers' throttle can be set on the first of their requests.
		const { browser: cold, port } = await debuggableBrowser();
		const workers = await throttleServiceWorkers(port);
		try {
			const withoutWorker: number[] = [];
			const timings: number[] = [];
			const installs: number[] = [];
			// The worker's install precaches the whole build beside the page's own requests, throttled too;
			// measured with and without it, interleaved so drift in the machine falls on both alike.
			for (let run = 0; run < 5; run++) {
				withoutWorker.push((await coldLoad(cold, session, 'block')).ms);
				const { ms, installed, throttled } = await coldLoad(cold, session, 'allow', workers);
				timings.push(ms);
				installs.push(installed!);
				// The worker's own fetches were throttled, or the run does not measure what it claims to.
				expect(throttled).toBe(1);
			}
			console.log(`hk 3G first load, service worker blocked: ${summary(withoutWorker)}`);
			console.log(`hk 3G first load: ${summary(timings)}`);
			console.log(`hk 3G service worker installed at: ${summary(installs)}`);
			expect(percentile(timings, 0.9)).toBeLessThan(2000);
		} finally {
			workers.stop();
			await cold.close();
		}
	});
});
