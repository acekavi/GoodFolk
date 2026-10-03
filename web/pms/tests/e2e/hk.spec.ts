import { expect, test, type Browser, type Page } from '@playwright/test';
import { addStaff, bookableHotel, createProperty, signIn, signUp } from './helpers';

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
		page.getByRole('group', { name: 'Rooms' }).getByRole('button', { name: /^101, Clean,/ })
	).toBeVisible();
	await expect(
		page.getByRole('group', { name: 'Rooms' }).getByRole('button', { name: /^102, Dirty,/ })
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
	await page.getByRole('group', { name: 'Rooms' }).getByRole('button', { name: /^101,/ }).click();
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
	await page.getByRole('group', { name: 'Rooms' }).getByRole('button', { name: /^101,/ }).click();
	await page.getByLabel('Set condition…').selectOption('INSPECTED');
	await expect(
		page.getByRole('group', { name: 'Rooms' }).getByRole('button', { name: /^101, Inspected,/ })
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
