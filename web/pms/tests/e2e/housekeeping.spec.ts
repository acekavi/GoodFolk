import { expect, test, type Browser, type Page } from '@playwright/test';
import {
	addDays,
	addStaff,
	bookableHotel,
	createProperty,
	insertAssignment,
	post,
	signIn,
	signUp
} from './helpers';

/** A hotel of rooms 101 to 104 with one stay arriving tonight in 101, and its owner's page on the board. */
async function board(page: Page) {
	const { email } = await signUp(page);
	await createProperty(page, 'HKB');
	const hotel = await bookableHotel(page, 4, 2);
	const propertyId = page.url().split('/p/')[1];
	const reservation = await post(page.request, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: hotel.businessDate,
				check_out: addDays(hotel.businessDate, 1),
				adults: 2
			}
		]
	});
	await page.goto(`/p/${propertyId}/housekeeping`);
	await expect(page.getByRole('heading', { name: 'Housekeeping' })).toBeVisible();
	return { email, propertyId, hotel, reservationId: reservation.id as string };
}

const tile = (page: Page, number: string) =>
	page
		.getByRole('grid', { name: 'Rooms' })
		.getByRole('button', { name: new RegExp(`^${number},`) });

/** Signs a staff user with `role` in, in a context of its own, on the board of `propertyId`. */
async function staffOnBoard(
	browser: Browser,
	ownerEmail: string,
	role: string,
	propertyId: string
) {
	const { email } = addStaff(ownerEmail, role, 'Front Desk Fiona');
	const context = await browser.newContext();
	const page = await context.newPage();
	await signIn(page, email);
	await page.goto(`/p/${propertyId}/housekeeping`);
	return { page, context };
}

test('the board shows every room with its condition and occupancy', async ({ page }) => {
	await board(page);

	await expect(page.getByRole('grid', { name: 'Rooms' }).getByRole('button')).toHaveCount(4);
	await expect(tile(page, '101')).toHaveAttribute('aria-label', /Due in/);
	await expect(tile(page, '102')).toHaveAttribute('aria-label', /Vacant/);
	await expect(tile(page, '101')).toHaveAttribute(
		'aria-label',
		/^101, (Dirty|Cleaning|Clean|Inspected),/
	);
});

test('the filters are kept in the URL', async ({ page }) => {
	await board(page);

	await page.getByLabel('Occupancy').selectOption('DUE_IN');

	await expect(page).toHaveURL(/occupancy=due_in/);
	await expect(page.getByRole('grid', { name: 'Rooms' }).getByRole('button')).toHaveCount(1);
	await page.reload();
	await expect(page.getByLabel('Occupancy')).toHaveValue('DUE_IN');
	await expect(tile(page, '101')).toBeVisible();
	await expect(tile(page, '102')).toHaveCount(0);
});

test('a bulk set to clean changes every selected room', async ({ page }) => {
	await board(page);

	// Vacant rooms start clean, so make all four dirty first.
	await tile(page, '101').click();
	await tile(page, '104').click({ modifiers: ['Shift'] });
	await page.getByLabel('Set condition…').selectOption('DIRTY');
	await expect(tile(page, '104')).toHaveAttribute('aria-label', /^104, Dirty,/);

	await tile(page, '101').click();
	await tile(page, '103').click({ modifiers: ['Shift'] });
	await expect(page.getByRole('status').filter({ hasText: 'selected' })).toHaveText('3 selected');
	await page.getByLabel('Set condition…').selectOption('CLEAN');

	for (const number of ['101', '102', '103']) {
		await expect(tile(page, number)).toHaveAttribute('aria-label', /^\d+, Clean,/);
	}
	await expect(tile(page, '104')).toHaveAttribute('aria-label', /^104, Dirty,/);
	await expect(page.getByRole('status').filter({ hasText: 'selected' })).toHaveText('0 selected');
});

test('rooms are assigned to a housekeeper, and Copy yesterday brings over the previous day', async ({
	page
}) => {
	const { email, propertyId, hotel } = await board(page);
	addStaff(email, 'housekeeping', 'Kumari Silva');
	await page.reload();
	// Kumari cleaned room 103 yesterday; the API only assigns for today, so this goes in as the schema owner.
	const me = await (await page.request.get('/api/v1/me')).json();
	const graphql = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ rooms(propertyId: "${propertyId}") { id number } housekeepers(propertyId: "${propertyId}") { userId displayName } }`
		}
	});
	const { rooms, housekeepers } = (await graphql.json()).data;
	insertAssignment({
		tenant: me.current_tenant,
		property: propertyId,
		date: addDays(hotel.businessDate, -1),
		room: rooms.find((room: { number: string }) => room.number === '103').id,
		user: housekeepers.find(
			(person: { displayName: string }) => person.displayName === 'Kumari Silva'
		).userId
	});

	await tile(page, '102').click();
	await page.getByLabel('Assign to…').selectOption({ label: 'Kumari Silva' });
	await expect(tile(page, '102')).toHaveAttribute('aria-label', /cleaned by Kumari Silva/);
	await expect(tile(page, '102').getByText('KS')).toBeVisible();

	await page.getByLabel('Housekeeper').selectOption({ label: 'Kumari Silva' });
	await expect(page.getByRole('grid', { name: 'Rooms' }).getByRole('button')).toHaveCount(1);
	await page.getByLabel('Housekeeper').selectOption('');

	await page.getByRole('button', { name: 'Copy yesterday' }).click();
	await expect(page.getByRole('status').filter({ hasText: 'Copied' })).toHaveText(
		'Copied 1 assignment from yesterday'
	);
	await expect(tile(page, '103')).toHaveAttribute('aria-label', /cleaned by Kumari Silva/);
	await expect(tile(page, '103').getByText('KS')).toBeVisible();
	await expect(tile(page, '102')).toHaveAttribute('aria-label', /cleaned by Kumari Silva/);

	await tile(page, '102').click();
	await page.getByLabel('Assign to…').selectOption('none');
	await expect(tile(page, '102')).not.toHaveAttribute('aria-label', /cleaned by/);
});

test('an issue is reported, shows as a badge, and is resolved', async ({ page }) => {
	await board(page);

	const form = page.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '103' });
	await form.getByLabel('Description').fill('Leaking tap');
	await form.getByRole('button', { name: 'Report issue' }).click();

	const issue = page.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
	await expect(issue).toContainText('Room 103');
	await expect(issue).toContainText('Leaking tap');
	await expect(tile(page, '103')).toHaveAttribute('aria-label', /1 open issue$/);
	await expect(issue.getByRole('button', { name: 'Block room' })).toHaveCount(0);

	await issue.getByRole('button', { name: 'Resolve' }).click();

	await expect(page.getByText('No open issues.')).toBeVisible();
	await expect(tile(page, '103')).not.toHaveAttribute('aria-label', /open issue/);
});

test('a high issue offers Block room, which opens the dialog for that room', async ({ page }) => {
	await board(page);

	const form = page.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '104' });
	await form.getByLabel('Severity').selectOption('HIGH');
	await form.getByLabel('Description').fill('Ceiling collapsed');
	await form.getByRole('button', { name: 'Report issue' }).click();
	await page.getByRole('button', { name: 'Block room' }).click();

	const dialog = page.getByRole('dialog', { name: 'Block a room' });
	await expect(dialog).toBeVisible();
	await expect(dialog.getByLabel('Room')).toHaveValue(/.+/);
	await expect(dialog.getByLabel('Room').locator('option:checked')).toContainText('104');
	await expect(dialog.getByLabel('Note')).toHaveValue('Ceiling collapsed');

	await dialog.getByLabel('Reason').selectOption({ index: 1 });
	await dialog.getByRole('button', { name: 'Block room' }).click();
	await expect(dialog).toBeHidden();
	await expect(tile(page, '104')).toHaveAttribute('aria-label', /out of (order|service)/);
});

test('a front desk user sees the board and can report, but has no supervisor actions', async ({
	page,
	browser
}) => {
	const { email, propertyId } = await board(page);
	const { page: desk, context } = await staffOnBoard(browser, email, 'front_desk', propertyId);

	// Without supervise, tiles are plain labelled elements, not buttons, in a read-only table whose scroller
	// the keyboard can reach to scroll it.
	const rooms = desk.getByRole('table', { name: 'Rooms' });
	await expect(desk.getByRole('grid', { name: 'Rooms' })).toHaveCount(0);
	await expect(rooms.getByRole('img')).toHaveCount(4);
	await expect(rooms.getByRole('cell')).toHaveCount(4);
	await expect(rooms.getByRole('button')).toHaveCount(0);
	await expect(rooms).toHaveAttribute('tabindex', '0');
	await rooms.focus();
	await expect(rooms).toBeFocused();
	await expect(desk.getByRole('form', { name: 'Report an issue' })).toBeVisible();
	await expect(desk.getByRole('button', { name: 'Copy yesterday' })).toHaveCount(0);
	await expect(desk.getByRole('toolbar', { name: 'Selected rooms' })).toHaveCount(0);

	const form = desk.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '101' });
	await form.getByLabel('Severity').selectOption('HIGH');
	await form.getByLabel('Description').fill('No hot water');
	await form.getByRole('button', { name: 'Report issue' }).click();
	const issue = desk.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
	await expect(issue).toContainText('No hot water');
	// Reporting includes attaching photos; resolving and blocking are the supervisor's.
	await expect(issue.getByRole('button')).toHaveText(['Add photos']);
	// The owner's board shows it without a reload: the `issues:` event refreshes the badge.
	await expect(tile(page, '101')).toHaveAttribute('aria-label', /1 open issue$/);

	await context.close();
});

/** Makes room 101 dirty through the API, as the signed-in user. */
async function makeDirty(page: Page, propertyId: string, hotel: { path: string }) {
	const board = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ housekeepingBoard(propertyId: "${propertyId}") { roomId number } }`
		}
	});
	const rooms: { roomId: string; number: string }[] = (await board.json()).data.housekeepingBoard;
	const response = await page.request.post(`${hotel.path}/rooms/conditions`, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data: { room_ids: [rooms.find((room) => room.number === '101')!.roomId], condition: 'dirty' }
	});
	expect(response.status(), await response.text()).toBeLessThan(300);
}

test('a dirty room refuses check-in, and a manager checks the guest in anyway with a reason', async ({
	page
}) => {
	const { propertyId, hotel, reservationId } = await board(page);
	await makeDirty(page, propertyId, hotel);

	await page.goto(`/p/${propertyId}/reservations/${reservationId}`);
	await expect(page.getByText('Condition', { exact: true })).toBeVisible();
	await expect(page.getByText('Dirty', { exact: true })).toBeVisible();
	await page.getByRole('button', { name: 'Check in', exact: true }).click();

	await expect(page.getByRole('alert').filter({ hasText: 'room 101 is Dirty' })).toBeVisible();
	const form = page.getByRole('form', { name: 'Check in anyway' });
	await expect(form.getByRole('button', { name: 'Check in anyway' })).toBeDisabled();
	await form.getByLabel('Reason').fill('Housekeeping is on its way');
	await form.getByRole('button', { name: 'Check in anyway' }).click();

	await expect(page.getByText('Checked in', { exact: true }).first()).toBeVisible();
	await expect(page.getByRole('button', { name: 'Undo check-in' })).toBeVisible();
	await expect(form).toBeHidden();
});

test('a front desk user is only told the room is dirty', async ({ page, browser }) => {
	const { email, propertyId, hotel, reservationId } = await board(page);
	await makeDirty(page, propertyId, hotel);
	const { page: desk, context } = await staffOnBoard(browser, email, 'front_desk', propertyId);

	await desk.goto(`/p/${propertyId}/reservations/${reservationId}`);
	await desk.getByRole('button', { name: 'Check in', exact: true }).click();

	await expect(desk.getByRole('alert').filter({ hasText: 'room 101 is Dirty' })).toBeVisible();
	await expect(desk.getByRole('form', { name: 'Check in anyway' })).toHaveCount(0);
	await expect(desk.getByRole('button', { name: 'Check in anyway' })).toHaveCount(0);
	await context.close();
});

test("the tape chart rail shows a room's condition and follows the board live", async ({
	page
}) => {
	const { propertyId, hotel } = await board(page);
	await makeDirty(page, propertyId, hotel);
	const tape = await page.context().newPage();
	await tape.goto(`/p/${propertyId}/tape`);
	const dot = tape
		.getByRole('list', { name: 'Rooms' })
		.getByRole('listitem')
		.filter({ hasText: '101' })
		.getByRole('img');
	await expect(dot).toHaveAttribute('title', 'Dirty');

	await tile(page, '101').click();
	await page.getByLabel('Set condition…').selectOption('CLEAN');

	await expect(dot).toHaveAttribute('title', 'Clean');
	await tape.close();
});

test('Block room is offered to a manager but not to a housekeeping supervisor', async ({
	page,
	browser
}) => {
	const { email, propertyId } = await board(page);
	const form = page.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '102' });
	await form.getByLabel('Severity').selectOption('HIGH');
	await form.getByLabel('Description').fill('Window will not close');
	await form.getByRole('button', { name: 'Report issue' }).click();

	// A supervisor resolves issues but, like the server's InventoryBlock grant, cannot block rooms.
	const supervisor = await staffOnBoard(browser, email, 'hk_supervisor', propertyId);
	const issue = supervisor.page.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
	await expect(issue).toContainText('Window will not close');
	await expect(issue.getByRole('button', { name: 'Resolve' })).toBeVisible();
	await expect(issue.getByRole('button', { name: 'Block room' })).toHaveCount(0);
	await supervisor.context.close();

	const manager = await staffOnBoard(browser, email, 'manager', propertyId);
	await expect(
		manager.page
			.getByRole('list', { name: 'Open issues' })
			.getByRole('listitem')
			.getByRole('button', { name: 'Block room' })
	).toBeVisible();
	await manager.context.close();
});

test('another user blocking a room shows its marker on the board without a reload', async ({
	page,
	browser
}) => {
	const { email, propertyId } = await board(page);
	const manager = await staffOnBoard(browser, email, 'manager', propertyId);
	const form = manager.page.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '103' });
	await form.getByLabel('Severity').selectOption('HIGH');
	await form.getByLabel('Description').fill('Flooded');
	await form.getByRole('button', { name: 'Report issue' }).click();
	await manager.page.getByRole('button', { name: 'Block room' }).click();
	const dialog = manager.page.getByRole('dialog', { name: 'Block a room' });
	await dialog.getByLabel('Reason').selectOption({ index: 1 });
	await dialog.getByRole('button', { name: 'Block room' }).click();
	await expect(dialog).toBeHidden();

	await expect(tile(page, '103')).toHaveAttribute('aria-label', /out of (order|service)/);
	await manager.context.close();
});

test.describe('a board of 500 rooms draws only the rooms in view', () => {
	test.describe.configure({ mode: 'serial' });
	const ROOMS = 500;
	let page: Page;

	test.beforeAll(async ({ browser }) => {
		test.setTimeout(120_000);
		page = await browser.newPage();
		await signUp(page);
		await createProperty(page, 'HKL');
		await bookableHotel(page, ROOMS, 2);
		const propertyId = page.url().split('/p/')[1];
		await page.goto(`/p/${propertyId}/housekeeping`);
		await expect(page.getByRole('heading', { name: 'Housekeeping' })).toBeVisible();
	});

	test.afterAll(async () => {
		await page.close();
	});

	const selection = () => page.getByRole('status').filter({ hasText: 'selected' });

	test('the DOM holds far fewer than 500 tiles, and the grid reports all of them', async () => {
		const grid = page.getByRole('grid', { name: 'Rooms' });
		await expect(tile(page, '101')).toBeVisible();
		const mounted = await grid.getByRole('button').count();
		expect(mounted).toBeGreaterThan(0);
		expect(mounted).toBeLessThan(150);
		expect(Number(await grid.getAttribute('aria-rowcount'))).toBe(
			Math.ceil(ROOMS / Number(await grid.getAttribute('aria-colcount')))
		);
		// One tab stop for the whole grid.
		await expect(grid.locator('button[tabindex="0"]')).toHaveCount(1);
		await expect(page.getByText(`${ROOMS} of ${ROOMS} rooms`)).toBeVisible();
	});

	test('the keyboard reaches a tile far down: it mounts and takes focus', async () => {
		await expect(tile(page, '600')).toHaveCount(0);
		await tile(page, '101').focus();
		await page.keyboard.press('Control+End');
		await expect(tile(page, '600')).toBeFocused();
		await expect(tile(page, '101')).toHaveCount(0);
		await page.keyboard.press('ArrowLeft');
		await expect(tile(page, '599')).toBeFocused();
		await page.keyboard.press('Control+Home');
		await expect(tile(page, '101')).toBeFocused();
		await page.keyboard.press('PageDown');
		await expect(page.locator('.tile:focus')).not.toHaveAttribute('aria-label', /^101,/);
		await page.keyboard.press('Space');
		await expect(selection()).toHaveText('1 selected');
		await page.keyboard.press('Space');
		await expect(selection()).toHaveText('0 selected');
	});

	test('a shift-click selects the range across rows that are not mounted', async () => {
		await tile(page, '101').scrollIntoViewIfNeeded();
		await tile(page, '101').click();
		await tile(page, '101').focus();
		await page.keyboard.press('Control+End');
		await expect(tile(page, '600')).toBeFocused();
		await tile(page, '600').click({ modifiers: ['Shift'] });
		await expect(selection()).toHaveText(`${ROOMS} selected`);
		await page.getByRole('button', { name: 'Clear' }).click();
		await expect(selection()).toHaveText('0 selected');
	});
});
