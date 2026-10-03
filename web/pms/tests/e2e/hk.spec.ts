import { expect, test, type Browser, type Page } from '@playwright/test';
import {
	PASSWORD,
	addStaff,
	bookableHotel,
	createProperty,
	rollBusinessDate,
	signIn,
	signUp
} from './helpers';

const csrf = { 'x-goodfolk-csrf': '1' };

/** A hotel of rooms 101 to 104, all dirty, with 101 and 102 assigned to the housekeeper Kumari. */
async function hotelWithHousekeeper(page: Page) {
	const { email } = await signUp(page);
	await createProperty(page, 'HKA');
	await bookableHotel(page, 4, 2);
	const propertyId = page.url().split('/p/')[1];
	const path = `/api/v1/properties/${propertyId}`;
	const { email: housekeeper } = addStaff(email, 'housekeeping', 'Kumari Silva');
	const graphql = await page.request.post('/graphql', {
		headers: csrf,
		data: {
			query: `{ rooms(propertyId: "${propertyId}") { id number } housekeepers(propertyId: "${propertyId}") { userId } }`
		}
	});
	const { rooms, housekeepers } = (await graphql.json()).data as {
		rooms: { id: string; number: string }[];
		housekeepers: { userId: string }[];
	};
	const dirtied = await page.request.post(`${path}/rooms/conditions`, {
		headers: { ...csrf, 'Idempotency-Key': crypto.randomUUID() },
		data: { room_ids: rooms.map((room) => room.id), condition: 'dirty' }
	});
	expect(dirtied.status(), await dirtied.text()).toBe(200);
	const assigned = await page.request.put(`${path}/hk-assignments`, {
		headers: csrf,
		data: {
			assignments: rooms
				.filter((room) => ['101', '102'].includes(room.number))
				.map((room) => ({ room_id: room.id, user_id: housekeepers[0].userId }))
		}
	});
	expect(assigned.status(), await assigned.text()).toBe(204);
	return { email, housekeeper, propertyId };
}

/** A phone-sized page of `email`, signed in. */
async function phone(browser: Browser, email: string) {
	const context = await browser.newContext({ viewport: { width: 375, height: 700 } });
	const page = await context.newPage();
	await signIn(page, email);
	return { page, context };
}

test('a housekeeper sees only their rooms and takes one from dirty to clean', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto('/hk');

	// The only property is chosen and kept in the URL.
	await expect(hk).toHaveURL(new RegExp(`/hk\\?property=${propertyId}`));
	const rooms = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem');
	await expect(rooms).toHaveCount(2);
	await expect(rooms.nth(0)).toContainText('101');
	await expect(rooms.nth(1)).toContainText('102');
	await expect(rooms.nth(0)).toContainText('Dirty');
	await expect(rooms.nth(0)).toContainText('DLX');
	await expect(rooms.nth(0)).toContainText('Vacant');
	await expect(rooms.nth(0).getByRole('button')).toHaveText('Start cleaning');
	expect((await rooms.nth(0).getByRole('button').boundingBox())!.height).toBeGreaterThanOrEqual(48);
	expect(await hk.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);

	await rooms.nth(0).getByRole('button', { name: 'Start cleaning' }).click();
	await expect(rooms.nth(0)).toContainText('Cleaning');
	await rooms.nth(0).getByRole('button', { name: 'Done' }).click();
	await expect(rooms.nth(0)).toContainText('Clean');
	await expect(rooms.nth(0).getByRole('button', { name: /Start cleaning|Done/ })).toHaveCount(0);

	await page.goto(`/p/${propertyId}/housekeeping`);
	await expect(
		page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101, Clean,/ })
	).toBeVisible();
	await expect(
		page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^102, Dirty,/ })
	).toBeVisible();
	await context.close();
});

test('a change made on the board shows in My rooms without a reload', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	const first = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem').first();
	await expect(first).toContainText('Dirty');

	await page.goto(`/p/${propertyId}/housekeeping`);
	await page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101,/ }).click();
	await page.getByLabel('Set condition…').selectOption('INSPECTED');

	await expect(first).toContainText('Inspected');
	await context.close();
});

test('a stale change is refused with a clear message and the list refreshes', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	// Without live events the list stays as it was read, so the supervisor's change leaves a stale button.
	await hk.route('**/api/v1/events*', (route) => route.abort());
	await hk.goto(`/hk?property=${propertyId}`);
	const first = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem').first();
	await expect(first).toContainText('Dirty');

	await page.goto(`/p/${propertyId}/housekeeping`);
	await page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101,/ }).click();
	await page.getByLabel('Set condition…').selectOption('INSPECTED');
	await expect(
		page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101, Inspected,/ })
	).toBeVisible();

	await first.getByRole('button', { name: 'Start cleaning' }).click();
	await expect(hk.getByRole('alert')).toContainText('Room 101');
	await expect(first).toContainText('Inspected');
	await context.close();
});

test('a housekeeper reports an issue', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);

	await hk.getByRole('link', { name: 'Report issue' }).first().click();
	// From My rooms the room is preselected; here the nav link leaves it to the housekeeper.
	const form = hk.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '103' });
	await form.getByLabel('Severity').selectOption('HIGH');
	await form.getByLabel('Description').fill('Leaking tap');
	await form.getByRole('button', { name: 'Report issue' }).click();
	await expect(hk.getByRole('status')).toHaveText('Issue reported for room 103.');
	await expect(form.getByLabel('Description')).toHaveValue('');

	await page.goto(`/p/${propertyId}/housekeeping`);
	const issue = page.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
	await expect(issue).toContainText('Room 103');
	await expect(issue).toContainText('Leaking tap');
	await context.close();
});

test('Report issue on a room preselects it', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);

	await hk
		.getByRole('list', { name: 'Rooms' })
		.getByRole('listitem')
		.nth(1)
		.getByRole('link', { name: 'Report issue' })
		.click();

	await expect(hk.getByLabel('Room').locator('option:checked')).toHaveText('102');
	await context.close();
});

test('a user without a housekeeping role is told so', async ({ page, browser }) => {
	const { email } = await hotelWithHousekeeper(page);
	const { email: accountant } = addStaff(email, 'accountant', 'Ann Accountant');
	const { page: other, context } = await phone(browser, accountant);
	await other.goto('/hk');

	await expect(other.getByRole('alert')).toHaveText(
		'Your account has no housekeeping role at any property.'
	);
	await context.close();
});

test('a signed-out visitor is sent to sign in', async ({ page }) => {
	await page.goto('/hk');
	await expect(page).toHaveURL(/\/login/);
});

/** Waits until the page's service worker is active, so the app shell is cached and works offline. */
async function shellCached(page: Page) {
	await page.evaluate(() => navigator.serviceWorker.ready.then(() => undefined));
}

test('the housekeeper app is installable', async ({ page }) => {
	await page.goto('/hk');
	await expect(page.locator('link[rel="manifest"]')).toHaveAttribute(
		'href',
		'/manifest.webmanifest'
	);
	const response = await page.request.get('/manifest.webmanifest');
	const manifest = await response.json();
	expect(manifest).toMatchObject({ start_url: '/hk', display: 'standalone' });
	for (const icon of manifest.icons) {
		expect((await page.request.get(icon.src)).headers()['content-type']).toContain('image/png');
	}
	expect(manifest.icons.map((icon: { sizes: string }) => icon.sizes)).toEqual(
		expect.arrayContaining(['192x192', '512x512'])
	);
	expect(manifest.icons.some((icon: { purpose?: string }) => icon.purpose === 'maskable')).toBe(
		true
	);
});

test('Start then Done offline is sent in order when the connection returns', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	const first = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem').first();
	await expect(first).toContainText('Dirty');
	await shellCached(hk);

	await context.setOffline(true);
	await expect(hk.getByText('You are offline.')).toBeVisible();
	await first.getByRole('button', { name: 'Start cleaning' }).click();
	await first.getByRole('button', { name: 'Done' }).click();
	await expect(first).toContainText('Clean');
	await expect(first).toContainText('Waiting to be sent');
	await expect(hk.getByText('2 changes waiting to be sent.')).toBeVisible();

	await context.setOffline(false);
	await expect(hk.getByRole('region', { name: 'Connection' })).toHaveCount(0);
	await expect(first).not.toContainText('Waiting to be sent');
	await expect(first).toContainText('Clean');

	await page.goto(`/p/${propertyId}/housekeeping`);
	await expect(
		page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101, Clean,/ })
	).toBeVisible();
	await context.close();
});

test('a change made stale while offline is a visible conflict, and the rest still go', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	const rooms = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem');
	await expect(rooms.first()).toContainText('Dirty');
	await shellCached(hk);

	await context.setOffline(true);
	await rooms.nth(0).getByRole('button', { name: 'Start cleaning' }).click();
	await rooms.nth(1).getByRole('button', { name: 'Start cleaning' }).click();

	await page.goto(`/p/${propertyId}/housekeeping`);
	await page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101,/ }).click();
	await page.getByLabel('Set condition…').selectOption('INSPECTED');
	await expect(
		page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101, Inspected,/ })
	).toBeVisible();

	await context.setOffline(false);
	const conflicts = hk.getByRole('list', { name: 'Conflicts' });
	await expect(conflicts).toContainText(
		'Room 101 was changed to Inspected by someone else; your Start cleaning was not applied.'
	);
	// Room 102 went through even though 101, ahead of it, did not.
	await expect(rooms.nth(1)).toContainText('Cleaning');
	await expect(rooms.nth(1)).not.toContainText('Waiting to be sent');

	await conflicts.getByRole('button', { name: 'Discard' }).click();
	await expect(hk.getByRole('region', { name: 'Connection' })).toHaveCount(0);
	await expect(rooms.nth(0)).toContainText('Inspected');
	await context.close();
});

test('a queue replayed after the day rolls shows what changed, sends the rest and refreshes the list', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const me = await (await page.request.get('/api/v1/me')).json();
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk/issue?property=${propertyId}`);
	const form = hk.getByRole('form', { name: 'Report an issue' });
	await expect(form.getByLabel('Room').locator('option')).toHaveCount(5);
	await hk.goto(`/hk?property=${propertyId}`);
	const rooms = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem');
	await expect(rooms).toHaveCount(2);
	await shellCached(hk);

	await context.setOffline(true);
	await hk.goto(`/hk/issue?property=${propertyId}`);
	await form.getByLabel('Room').selectOption({ label: '103' });
	await form.getByLabel('Description').fill('Cracked window');
	await form.getByRole('button', { name: 'Report issue' }).click();
	await expect(hk.getByText('saved on this device')).toBeVisible();
	await hk.goto(`/hk?property=${propertyId}`);
	await rooms.nth(0).getByRole('button', { name: 'Start cleaning' }).click();
	await expect(hk.getByText('2 changes waiting to be sent.')).toBeVisible();

	// Night audit while the phone is offline: tomorrow nobody is assigned yet.
	rollBusinessDate(me.current_tenant, propertyId);
	await context.setOffline(false);

	const conflicts = hk.getByRole('list', { name: 'Conflicts' });
	await expect(conflicts).toContainText('Your rooms changed since you went offline.');
	await expect(conflicts).toContainText('Room 101');
	await expect(hk.getByText('No rooms are assigned to you today.')).toBeVisible();
	await expect(hk.getByText('waiting to be sent')).toHaveCount(0);

	// The report was not tied to an assignment, so it went through behind the refused change.
	await expect(conflicts.getByRole('listitem')).toHaveCount(1);

	await page.goto(`/p/${propertyId}/housekeeping`);
	await expect(page.getByRole('list', { name: 'Open issues' }).getByRole('listitem')).toContainText(
		'Cracked window'
	);
	await conflicts.getByRole('button', { name: 'Discard' }).click();
	await expect(hk.getByRole('region', { name: 'Connection' })).toHaveCount(0);
	await context.close();
});

test('a report made offline is sent once when the connection returns', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk/issue?property=${propertyId}`);
	const form = hk.getByRole('form', { name: 'Report an issue' });
	await expect(form.getByLabel('Room').locator('option')).toHaveCount(5);
	await shellCached(hk);

	await context.setOffline(true);
	await form.getByLabel('Room').selectOption({ label: '103' });
	await form.getByLabel('Description').fill('Cracked window');
	await form.getByRole('button', { name: 'Report issue' }).click();
	await expect(hk.getByText('saved on this device')).toBeVisible();
	await expect(hk.getByText('1 change waiting to be sent.')).toBeVisible();

	await context.setOffline(false);
	await expect(hk.getByRole('region', { name: 'Connection' })).toHaveCount(0);

	await page.goto(`/p/${propertyId}/housekeeping`);
	const issue = page.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
	await expect(issue).toHaveCount(1);
	await expect(issue).toContainText('Cracked window');
	await context.close();
});

test('a reload offline still shows My rooms, labelled with its time', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	await expect(hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem')).toHaveCount(2);
	await shellCached(hk);
	// Let the saved list reach IndexedDB before the connection goes.
	await expect
		.poll(() =>
			hk.evaluate(
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
	await hk.goto('/hk');
	const rooms = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem');
	await expect(rooms).toHaveCount(2);
	await expect(rooms.nth(0)).toContainText('101');
	await expect(rooms.nth(0)).toContainText('DLX');
	await expect(hk.getByText(/^Showing the list as it was on .+ at /)).toBeVisible();
	await expect(hk.getByText('You are offline.')).toBeVisible();
	await context.close();
});

test('the service worker never caches the API or GraphQL', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	await expect(hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem')).toHaveCount(2);
	await shellCached(hk);
	const cached = await hk.evaluate(async () => {
		const urls: string[] = [];
		for (const name of await caches.keys()) {
			for (const request of await (await caches.open(name)).keys()) urls.push(request.url);
		}
		return urls;
	});
	expect(cached.length).toBeGreaterThan(0);
	expect(cached.filter((url) => /\/(api|graphql)(\/|\?|$)/.test(new URL(url).pathname))).toEqual(
		[]
	);
	await context.close();
});

test("another user on the device gets neither the first one's changes nor their list", async ({
	page,
	browser
}) => {
	const { email, housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { email: second } = addStaff(email, 'housekeeping', 'Nimali Fernando');
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	const first = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem').first();
	await expect(first).toContainText('Dirty');
	await shellCached(hk);

	await context.setOffline(true);
	await first.getByRole('button', { name: 'Start cleaning' }).click();
	await expect(hk.getByText('1 change waiting to be sent.')).toBeVisible();
	await context.setOffline(false);
	await expect(hk.getByRole('region', { name: 'Connection' })).toHaveCount(0);

	// A second change is left waiting, and the first user's session ends without signing out.
	await context.setOffline(true);
	await first.getByRole('button', { name: 'Done' }).click();
	await expect(hk.getByText('1 change waiting to be sent.')).toBeVisible();
	await context.clearCookies();
	await context.setOffline(false);

	// The second user signs in on the same device: the change is theirs to neither send nor see.
	await signIn(hk, second);
	await hk.goto('/hk');
	await expect(hk.getByText('Saved by another user on this device: 1 (not sent).')).toBeVisible();
	await expect(hk.getByText('1 change waiting to be sent.')).toHaveCount(0);
	await hk.getByRole('button', { name: 'Discard all' }).click();
	await expect(hk.getByRole('region', { name: 'Connection' })).toHaveCount(0);
	await context.close();
});

test('signing out warns about unsent changes, then clears the device', async ({
	page,
	browser
}) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	const first = hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem').first();
	await expect(first).toContainText('Dirty');
	await shellCached(hk);
	// Online, but the server cannot be reached for condition changes: they queue and stay queued.
	await hk.route('**/rooms/*/condition', (route) => route.abort());
	await first.getByRole('button', { name: 'Start cleaning' }).click();
	await first.getByRole('button', { name: 'Done' }).click();
	await expect(hk.getByText('2 changes waiting to be sent.')).toBeVisible();

	const messages: string[] = [];
	hk.once('dialog', async (dialog) => {
		messages.push(dialog.message());
		await dialog.dismiss();
	});
	await hk.getByRole('button', { name: 'Sign out' }).click();
	await expect.poll(() => messages.length).toBe(1);
	expect(messages[0]).toMatch(/2 changes haven't been sent and will be lost/);
	await expect(hk).toHaveURL(/\/hk\?/);

	hk.once('dialog', (dialog) => dialog.accept());
	await hk.getByRole('button', { name: 'Sign out' }).click();
	await expect(hk).toHaveURL(/\/login/);
	const left = await hk.evaluate(
		() =>
			new Promise<number>((done) => {
				const open = indexedDB.open('goodfolk-hk');
				open.onsuccess = () => {
					const transaction = open.result.transaction(['kv', 'queue']);
					let total = 0;
					for (const name of ['kv', 'queue']) {
						const count = transaction.objectStore(name).count();
						count.onsuccess = () => {
							total += count.result;
						};
					}
					transaction.oncomplete = () => done(total);
				};
			})
	);
	expect(left).toBe(0);
	await context.close();
});

test('a 401 never shows the saved list', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	await expect(hk.getByRole('list', { name: 'Rooms' }).getByRole('listitem')).toHaveCount(2);
	await expect
		.poll(() =>
			hk.evaluate(
				() =>
					new Promise<number>((done) => {
						const open = indexedDB.open('goodfolk-hk');
						open.onsuccess = () => {
							const count = open.result.transaction('kv').objectStore('kv').count();
							count.onsuccess = () => done(count.result);
						};
					})
			)
		)
		.toBeGreaterThan(1);

	await hk.route('**/graphql', (route) =>
		route.fulfill({
			status: 401,
			contentType: 'application/json',
			body: JSON.stringify({ type: 'about:blank', title: 'Unauthorized', status: 401 })
		})
	);
	await hk.reload();
	await expect(hk.getByRole('alert')).toBeVisible();
	await expect(hk.getByRole('list', { name: 'Rooms' })).toHaveCount(0);
	await expect(hk.getByText(/^Showing the list as it was/)).toHaveCount(0);
	await context.close();
});

test("A's waiting changes are not sent when another window has signed in as B", async ({
	page,
	browser
}) => {
	const { email, housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { email: second } = addStaff(email, 'housekeeping', 'Nimali Fernando');
	const { page: one, context } = await phone(browser, housekeeper);
	await one.goto(`/hk?property=${propertyId}`);
	const first = one.getByRole('list', { name: 'Rooms' }).getByRole('listitem').first();
	await expect(first).toContainText('Dirty');
	await shellCached(one);

	await context.setOffline(true);
	await first.getByRole('button', { name: 'Start cleaning' }).click();
	await expect(one.getByText('1 change waiting to be sent.')).toBeVisible();
	// A's session ends; the change stays, asking to sign in.
	await context.clearCookies();
	await context.setOffline(false);
	await expect(one.getByText(/Your session has expired/)).toBeVisible();

	// In a second window of the same browser, B signs in. Window one still believes A is signed in.
	const two = await context.newPage();
	await signIn(two, second);
	await one.evaluate(() => window.dispatchEvent(new Event('focus')));

	await expect(one.getByText('Saved by another user on this device: 1 (not sent).')).toBeVisible();
	await page.goto(`/p/${propertyId}/housekeeping`);
	await expect(
		page.getByRole('grid', { name: 'Rooms' }).getByRole('button', { name: /^101, Dirty,/ })
	).toBeVisible();
	await context.close();
});

test('after signing in again the housekeeper is back on /hk', async ({ page, browser }) => {
	const { housekeeper, propertyId } = await hotelWithHousekeeper(page);
	const { page: hk, context } = await phone(browser, housekeeper);
	await hk.goto(`/hk?property=${propertyId}`);
	await expect(hk.getByRole('list', { name: 'Rooms' })).toBeVisible();
	await context.clearCookies();
	await hk.reload();
	await expect(hk).toHaveURL(/\/login\?redirect=/);
	await hk.getByLabel('Email').fill(housekeeper);
	await hk.getByLabel('Password').fill(PASSWORD);
	await hk.getByRole('button', { name: 'Sign in' }).click();
	await expect(hk).toHaveURL(new RegExp(`/hk\\?property=${propertyId}`));
	await context.close();
});
