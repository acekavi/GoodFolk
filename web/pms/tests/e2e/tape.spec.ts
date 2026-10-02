import { expect, test, type Page } from '@playwright/test';
import {
	addDays,
	bookableHotel,
	createProperty,
	PASSWORD,
	post,
	runsQuery,
	signUp,
	unassign,
	type Hotel
} from './helpers';

/** Rooms 101 to 106 of DLX (from `bookableHotel`) and 201 to 206 of STD: twelve rooms on two pages. */
async function twelveRooms(page: Page): Promise<Hotel> {
	await signUp(page);
	await createProperty(page, 'TPE');
	const hotel = await bookableHotel(page, 6, 3);
	const standard = await post(page.request, `${hotel.path}/room-types`, {
		code: 'STD',
		name: 'Standard',
		base_occupancy: 2,
		max_adults: 2,
		max_children: 1,
		max_occupancy: 3
	});
	await post(page.request, `${hotel.path}/rooms/bulk`, {
		room_type_id: standard.id,
		first: 201,
		last: 206
	});
	return hotel;
}

/** Books one DLX room for the business date's night; returns its reservation and room to act on. */
async function bookTonight(page: Page, hotel: Hotel) {
	const created = await post(page.request, `${hotel.path}/reservations`, {
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
	return {
		id: created.id as string,
		roomId: created.rooms[0].id as string,
		version: created.rooms[0].version as number
	};
}

const rows = (page: Page) =>
	page.getByRole('list', { name: 'Rooms', exact: true }).getByRole('listitem');
const picker = (page: Page) => page.getByRole('combobox', { name: 'Pick rooms' });

async function pick(page: Page, text: string) {
	await picker(page).fill(text);
	await picker(page).press('Enter');
}

test('the chart pages ten rooms at a time, narrows with chips and keeps its view in the URL', async ({
	page
}) => {
	const hotel = await twelveRooms(page);
	await page.getByRole('link', { name: 'Tape chart' }).click();

	await expect(rows(page)).toHaveCount(10);
	await expect(page.getByText('Rooms 1–10 of 12')).toBeVisible();
	// The opening view starts two days before the business date.
	await expect(page.getByRole('group', { name: 'Tape chart' })).toHaveAttribute(
		'data-start',
		addDays(hotel.businessDate, -2)
	);
	await page.getByRole('button', { name: 'Next' }).click();
	await expect(rows(page)).toHaveCount(2);
	await expect(page.getByText('Rooms 11–12 of 12')).toBeVisible();
	await page.getByRole('button', { name: 'Prev' }).click();
	await expect(rows(page)).toHaveCount(10);

	// A type chip narrows to that type; a range chip to the rooms between its ends.
	await pick(page, 'STD');
	await expect(page.getByRole('button', { name: 'Remove STD' })).toBeVisible();
	await expect(rows(page)).toHaveCount(6);
	await expect(rows(page).first()).toContainText('201');
	await page.getByRole('button', { name: 'Remove STD' }).click();
	await expect(rows(page)).toHaveCount(10);
	await pick(page, '101-103');
	await expect(rows(page)).toHaveCount(3);
	await picker(page).press('Backspace');
	await expect(rows(page)).toHaveCount(10);

	// Reloading keeps the chips and the page.
	await pick(page, '101-206');
	await page.getByRole('button', { name: 'Next' }).click();
	await expect(page.getByText('Rooms 11–12 of 12')).toBeVisible();
	await page.reload();
	await expect(page.getByRole('button', { name: 'Remove 101-206' })).toBeVisible();
	await expect(page.getByText('Rooms 11–12 of 12')).toBeVisible();
	await expect(rows(page)).toHaveCount(2);
});

test('a stay opens over the chart, which stays mounted with its scroll and focus; Escape comes back', async ({
	page
}) => {
	const hotel = await twelveRooms(page);
	const { id } = await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const chart = page.getByRole('group', { name: 'Tape chart' });
	const bar = page.locator('[data-room="101"]', { hasText: 'Silva, A.' });
	await expect(bar).toBeVisible();

	// Move the focus off its starting cell and scroll, so a remount would show.
	await chart.focus();
	await page.keyboard.press('ArrowDown');
	await page.keyboard.press('ArrowDown');
	const opening = await chart.getAttribute('data-start');
	await chart.evaluate((el) => (el.scrollLeft += (el.clientWidth - 112) / 12));
	await expect(chart).not.toHaveAttribute('data-start', opening!);
	const start = await chart.getAttribute('data-start');
	const focused = await chart.getByRole('status').textContent();
	expect(focused).toContain('103');

	const lists: string[] = [];
	page.on('request', (request) => {
		if (runsQuery(request, 'ReservationList')) lists.push(request.url());
	});
	await bar.click();
	await expect(page).toHaveURL(new RegExp(`/reservations/${id}`));
	const dialog = page.getByRole('dialog');
	await expect(dialog).toBeVisible();
	// The chart is still in the page, under the dialog, and no list was loaded behind it.
	await expect(chart).toBeVisible();
	await expect(bar).toBeVisible();
	expect(lists).toEqual([]);

	await page.keyboard.press('Escape');
	await expect(dialog).toBeHidden();
	await expect(page).toHaveURL(/\/tape(\?|$)/);
	await expect(chart).toHaveAttribute('data-start', start!);
	await expect(chart.getByRole('status')).toHaveText(focused!);
	await expect(chart).toBeVisible();

	// A reload of the reservation's URL still opens it, over the reservations list.
	await bar.click();
	await expect(dialog).toBeVisible();
	await page.reload();
	await expect(page.getByRole('dialog')).toBeVisible();
	await expect(page.getByRole('table', { name: 'Reservations' })).toBeVisible();
});

test('the arrow keys move the focused day, and Enter opens the bar under it', async ({ page }) => {
	const hotel = await twelveRooms(page);
	const { id } = await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();

	const chart = page.getByRole('group', { name: 'Tape chart' });
	await chart.focus();
	// The focus starts on the first room, on the business date.
	await expect(chart.getByRole('status')).toContainText(`101 ${hotel.businessDate}`);
	await page.keyboard.press('ArrowRight');
	await expect(chart.getByRole('status')).toContainText(`101 ${addDays(hotel.businessDate, 1)}`);
	await page.keyboard.press('ArrowLeft');
	await page.keyboard.press('ArrowDown');
	await expect(chart.getByRole('status')).toContainText(`102 ${hotel.businessDate}`);
	await page.keyboard.press('ArrowUp');
	await page.keyboard.press('Enter');
	await expect(page).toHaveURL(new RegExp(`/reservations/${id}`));
	await expect(page.getByRole('dialog')).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(chart).toBeFocused();
	await expect(chart.getByRole('status')).toContainText(`101 ${hotel.businessDate}`);

	// PageDown and PageUp page the rooms.
	await page.keyboard.press('PageDown');
	await expect(page.getByText('Rooms 11–12 of 12')).toBeVisible();
	await expect(rows(page)).toHaveCount(2);
	await page.keyboard.press('PageUp');
	await expect(page.getByText('Rooms 1–10 of 12')).toBeVisible();
	await expect(rows(page)).toHaveCount(10);
});

test('T and Today win over a scroll that is still settling', async ({ page }) => {
	const hotel = await twelveRooms(page);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const chart = page.getByRole('group', { name: 'Tape chart' });
	const opening = addDays(hotel.businessDate, -2);
	await expect(chart).toHaveAttribute('data-start', opening);
	await chart.focus();

	for (const jump of ['key', 'button'] as const) {
		await chart.evaluate((el) => (el.scrollLeft += el.clientWidth * 2));
		await expect(chart).not.toHaveAttribute('data-start', opening);
		// Within the settle pause, before the scrolled start reaches the URL.
		if (jump === 'key') await page.keyboard.press('t');
		else await page.getByRole('button', { name: 'Today' }).click();
		await page.waitForTimeout(500);
		await expect(chart).toHaveAttribute('data-start', opening);
		await expect(page).toHaveURL(new RegExp(`start=${opening}`));
	}
});

test('Today after scrolling a year away brings the business date back', async ({ page }) => {
	const hotel = await twelveRooms(page);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const chart = page.getByRole('group', { name: 'Tape chart' });
	const opening = addDays(hotel.businessDate, -2);
	await expect(chart).toHaveAttribute('data-start', opening);

	await chart.evaluate((el) => (el.scrollLeft = el.scrollWidth));
	await expect(chart).not.toHaveAttribute('data-start', opening);
	await page.getByRole('button', { name: 'Today' }).click();
	await expect(chart).toHaveAttribute('data-start', opening);
});

test('a property with five rooms has no picker and no paging', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'FIV');
	await bookableHotel(page, 5, 1);
	await page.getByRole('link', { name: 'Tape chart' }).click();

	await expect(rows(page)).toHaveCount(5);
	await expect(picker(page)).toHaveCount(0);
	await expect(page.getByRole('button', { name: 'Next' })).toHaveCount(0);
	await expect(page.getByText(/Rooms 1/)).toHaveCount(0);
});

test('another session sees a booking appear and a cancellation disappear without reloading', async ({
	page,
	browser
}) => {
	const { email } = await (async () => {
		const signedUp = await signUp(page);
		await createProperty(page, 'LIV');
		return signedUp;
	})();
	const hotel = await bookableHotel(page, 4, 1);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(rows(page)).toHaveCount(4);
	const tapeUrl = page.url();

	const other = await browser.newContext({ baseURL: new URL(tapeUrl).origin });
	const watcher = await other.newPage();
	await watcher.goto('/login');
	await watcher.getByLabel('Email').fill(email);
	await watcher.getByLabel('Password').fill(PASSWORD);
	await watcher.getByRole('button', { name: 'Sign in' }).click();
	await expect(watcher.getByRole('heading', { name: 'Properties' })).toBeVisible();
	await watcher.goto(tapeUrl);
	await expect(rows(watcher)).toHaveCount(4);
	const bars = watcher.locator('[data-room]', { hasText: 'Silva, A.' });
	await expect(bars).toHaveCount(0);

	const { roomId, version } = await bookTonight(page, hotel);
	await expect(bars).toHaveCount(1);

	const cancelled = await page.request.post(`${hotel.path}/reservation-rooms/${roomId}/cancel`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${version}"` }
	});
	expect(cancelled.status(), await cancelled.text()).toBe(200);
	await expect(bars).toHaveCount(0);
	await other.close();
});

test('scrolling keeps the first day in the URL and loads the tiles it reveals; Today comes back', async ({
	page
}) => {
	const hotel = await twelveRooms(page);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const chart = page.getByRole('group', { name: 'Tape chart' });
	const opening = addDays(hotel.businessDate, -2);
	await expect(chart).toHaveAttribute('data-start', opening);

	const revealed = page.waitForRequest((request) => runsQuery(request, 'TapeWindow'));
	await chart.evaluate((el) => (el.scrollLeft += el.clientWidth * 3));
	await revealed;
	await expect(chart).not.toHaveAttribute('data-start', opening);
	const later = await chart.getAttribute('data-start');
	await expect(page).toHaveURL(new RegExp(`start=${later}`));

	await page.getByRole('button', { name: 'Today' }).click();
	await expect(chart).toHaveAttribute('data-start', opening);
	await expect(page).toHaveURL(new RegExp(`start=${opening}`));
	await page.getByRole('button', { name: '30 days' }).click();
	await expect(page).toHaveURL(/span=30/);
	await expect(chart).toHaveAttribute('data-start', opening);
});

test('hovering Next loads the next page of rooms before it is clicked', async ({ page }) => {
	await twelveRooms(page);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(rows(page)).toHaveCount(10);

	let requested = 0;
	page.on('request', (request) => {
		if (runsQuery(request, 'TapeWindow')) requested++;
	});
	await page.getByRole('button', { name: 'Next' }).hover();
	await expect.poll(() => requested).toBeGreaterThan(0);
	await page.waitForTimeout(500);
	const prefetched = requested;
	await page.getByRole('button', { name: 'Next' }).click();
	await expect(rows(page)).toHaveCount(2);
	await expect(page.getByText('Rooms 11–12 of 12')).toBeVisible();
	await page.waitForTimeout(500);
	// Only the tile of overscan ahead of the scroll is new; the tiles in view were prefetched.
	expect(requested - prefetched).toBeLessThanOrEqual(1);
});

/** Lets `hotel`'s room type be sold `allowance` rooms beyond its physical count. */
async function overbook(page: Page, hotel: Hotel, allowance: number) {
	const propertyId = hotel.path.split('/').pop();
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ roomTypes(propertyId: "${propertyId}") { id version } }`
		}
	});
	const { data } = await response.json();
	const type = data.roomTypes.find(
		(candidate: { id: string }) => candidate.id === hotel.roomTypeId
	);
	const patched = await page.request.patch(`${hotel.path}/room-types/${hotel.roomTypeId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${type.version}"` },
		data: { overbooking: allowance }
	});
	expect(patched.status(), await patched.text()).toBe(200);
}

const silva = (page: Page) => page.locator('[data-room]', { hasText: 'Silva, A.' });

test('an overbooked stay is listed under Needs a room, and Assign… puts it in a freed room', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'NRM');
	const hotel = await bookableHotel(page, 2, 1);
	await overbook(page, hotel, 1);
	const first = await bookTonight(page, hotel);
	await bookTonight(page, hotel);
	await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(silva(page)).toHaveCount(2);

	const needs = page.getByRole('button', { name: /^Needs a room/ });
	await expect(needs).toHaveText('Needs a room (1)');
	await needs.click();
	const panel = page.getByRole('region', { name: 'Needs a room' });
	await expect(panel).toContainText('Silva, A.');
	await expect(panel).toContainText('DLX');
	await expect(panel).toContainText('Overbooked');

	const cancelled = await page.request.post(
		`${hotel.path}/reservation-rooms/${first.roomId}/cancel`,
		{
			headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${first.version}"` }
		}
	);
	expect(cancelled.status(), await cancelled.text()).toBe(200);
	await expect(silva(page)).toHaveCount(1);

	await panel.getByRole('button', { name: 'Assign…' }).click();
	await panel.getByRole('combobox').selectOption({ label: '101' });
	await panel.getByRole('button', { name: 'Assign', exact: true }).click();
	await expect(needs).toHaveCount(0);
	await expect(silva(page)).toHaveCount(2);
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
});

test('Move to room… moves a bar to a room on the next page', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'MOV');
	const hotel = await bookableHotel(page, 12, 1);
	await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const bar = page.locator('[data-room="101"]', { hasText: 'Silva, A.' });
	await expect(bar).toBeVisible();

	await bar.hover();
	await page.getByRole('button', { name: 'Menu for Silva, A.' }).click();
	await page.getByRole('menuitem', { name: 'Move to room…' }).click();
	const dialog = page.getByRole('dialog', { name: 'Move to room' });
	await dialog.getByRole('combobox').selectOption({ label: '111' });
	await dialog.getByRole('button', { name: 'Move', exact: true }).click();
	await expect(dialog).toBeHidden();
	await expect(bar).toHaveCount(0);

	await page.getByRole('button', { name: 'Next' }).click();
	await expect(page.locator('[data-room="111"]', { hasText: 'Silva, A.' })).toBeVisible();
});

test('the bar menu opens from the keyboard, Enter chooses Move to room…, and Escape closes it', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'KBD');
	const hotel = await bookableHotel(page, 12, 1);
	await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	const chart = page.getByRole('group', { name: 'Tape chart' });
	const tapeUrl = page.url();

	await chart.focus();
	await page.keyboard.press('ContextMenu');
	const item = page.getByRole('menuitem', { name: 'Move to room…' });
	await expect(item).toBeFocused();
	await page.keyboard.press('Escape');
	await expect(item).toHaveCount(0);
	await expect(chart).toBeFocused();

	await page.keyboard.press('Shift+F10');
	await expect(item).toBeFocused();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('dialog', { name: 'Move to room' })).toBeVisible();
	// Enter chose the menu item; it did not open the reservation.
	expect(page.url()).toBe(tapeUrl);
});

test('a room taken after the picker opened is refused with the server reason, and the picker refreshes', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'CNF');
	const hotel = await bookableHotel(page, 1, 1);
	// The unassigned stay still counts as sold, so the second booking needs an allowance.
	await overbook(page, hotel, 1);
	const stay = await bookTonight(page, hotel);
	await unassign(page.request, hotel, stay.roomId, stay.version);
	await page.getByRole('link', { name: 'Tape chart' }).click();

	await page.getByRole('button', { name: /^Needs a room/ }).click();
	const panel = page.getByRole('region', { name: 'Needs a room' });
	await panel.getByRole('button', { name: 'Assign…' }).click();
	await panel.getByRole('combobox').selectOption({ label: '101' });

	// Another booking takes room 101 while the picker is open.
	await bookTonight(page, hotel);
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	await panel.getByRole('button', { name: 'Assign', exact: true }).click();

	await expect(panel.getByRole('alert')).toContainText('101');
	await expect(panel.getByText('No DLX room is free for these nights.')).toBeVisible();
	await expect(panel.getByRole('button', { name: 'Assign…' })).toBeVisible();
});

test('without manageReservations the chart has no Assign… and no bar menu', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'ROL');
	const hotel = await bookableHotel(page, 2, 1);
	await overbook(page, hotel, 1);
	await bookTonight(page, hotel);
	await bookTonight(page, hotel);
	await bookTonight(page, hotel);
	// The session as a housekeeper sees it; the API itself still enforces the permission.
	await page.route('**/api/v1/me', async (route) => {
		const response = await route.fetch();
		const me = await response.json();
		me.grants = me.grants.map((grant: object) => ({ ...grant, role: 'housekeeping' }));
		await route.fulfill({ response, json: me });
	});
	await page.reload();
	await page.getByRole('link', { name: 'Tape chart' }).click();

	await expect(silva(page)).toHaveCount(2);
	await expect(page.getByRole('group', { name: 'Tape chart' })).toHaveAttribute(
		'data-editable',
		'false'
	);
	await silva(page).first().hover();
	await expect(page.getByRole('button', { name: /^Menu for/ })).toHaveCount(0);
	await silva(page).first().click({ button: 'right' });
	await expect(page.getByRole('menu')).toHaveCount(0);

	// The list is still readable, but nothing in it assigns.
	await page.getByRole('button', { name: /^Needs a room/ }).click();
	await expect(page.getByRole('region', { name: 'Needs a room' })).toContainText('Overbooked');
	await expect(page.getByRole('button', { name: 'Assign…' })).toHaveCount(0);
});

/** Presses on `from`, moves to `to` in steps (a drag is more than its two ends) and releases. */
async function drag(page: Page, from: { x: number; y: number }, to: { x: number; y: number }) {
	await page.mouse.move(from.x, from.y);
	await page.mouse.down();
	const steps = 8;
	for (let step = 1; step <= steps; step++) {
		await page.mouse.move(
			from.x + ((to.x - from.x) * step) / steps,
			from.y + ((to.y - from.y) * step) / steps
		);
	}
	await page.mouse.up();
}

/** The bar of room `number`, dragged one row down by its body to the next room. */
async function dragToNextRoom(page: Page, number: string) {
	const box = await page.locator(`[data-room="${number}"]`, { hasText: 'Silva, A.' }).boundingBox();
	expect(box).not.toBeNull();
	const x = box!.x + box!.width / 2;
	const y = box!.y + box!.height / 2;
	await drag(page, { x, y }, { x, y: y + 44 });
}

/** Another session assigning the stay to `roomNumber`, over the API; returns the response status. */
async function assignTo(
	page: Page,
	hotel: Hotel,
	stayId: string,
	version: number,
	roomNumber: string
): Promise<number> {
	const propertyId = hotel.path.split('/').pop();
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { query: `{ rooms(propertyId: "${propertyId}") { id number } }` }
	});
	const { data } = await response.json();
	const room = data.rooms.find((candidate: { number: string }) => candidate.number === roomNumber);
	const assigned = await page.request.post(`${hotel.path}/reservation-rooms/${stayId}/assign`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${version}"` },
		data: { room_id: room.id }
	});
	return assigned.status();
}

test('dragging a bar to another room of its type saves at once, and Undo puts it back', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'DRG');
	const hotel = await bookableHotel(page, 6, 1);
	await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	const tapeUrl = page.url();

	await dragToNextRoom(page, '101');
	await expect(page.locator('[data-room="102"]', { hasText: 'Silva, A.' })).toBeVisible();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toHaveCount(0);
	await expect(page.getByText('Moved to 102')).toBeVisible();
	// The drag did not open the stay.
	expect(page.url()).toBe(tapeUrl);

	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	await expect(page.locator('[data-room="102"]', { hasText: 'Silva, A.' })).toHaveCount(0);
});

test('a drag on data another session already changed rolls back with the server reason', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'STL');
	const hotel = await bookableHotel(page, 6, 1);
	const stay = await bookTonight(page, hotel);
	// This session keeps showing the chart as it first loaded, as if it had missed the live update.
	const frozen = new Map<string, string>();
	let freeze = false;
	await page.route('**/graphql', async (route) => {
		const request = route.request();
		if (!runsQuery(request, 'TapeWindow')) return route.continue();
		const key = request.postData() ?? '';
		if (freeze && frozen.has(key)) {
			return route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: frozen.get(key)!
			});
		}
		const response = await route.fetch();
		const body = await response.text();
		frozen.set(key, body);
		await route.fulfill({ response, body });
	});
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	freeze = true;

	expect(await assignTo(page, hotel, stay.roomId, stay.version, '103')).toBe(200);
	// Nothing refetches from here on, so the bar can only return to 101 by the rollback itself.
	let release = () => {};
	const held = new Promise<void>((resolve) => (release = resolve));
	await page.route('**/graphql', async (route) => {
		if (!runsQuery(route.request(), 'TapeWindow')) return route.fallback();
		await held;
		await route.fallback();
	});
	await dragToNextRoom(page, '101');

	await expect(page.getByText(/changed by someone else/)).toBeVisible();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	await expect(page.locator('[data-room="102"]', { hasText: 'Silva, A.' })).toHaveCount(0);
	release();
});

test('Undo after the stay changed again shows the server reason and does not retry', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'UND');
	const hotel = await bookableHotel(page, 6, 1);
	const stay = await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();

	await dragToNextRoom(page, '101');
	await expect(page.getByText('Moved to 102')).toBeVisible();
	// Someone else moves it on, using the version the drag produced.
	expect(await assignTo(page, hotel, stay.roomId, stay.version + 1, '104')).toBe(200);
	await expect(page.locator('[data-room="104"]', { hasText: 'Silva, A.' })).toBeVisible();

	let assigns = 0;
	await page.route('**/assign', (route) => {
		assigns++;
		return route.continue();
	});
	await page.getByRole('button', { name: 'Undo' }).click();
	await expect(page.getByText(/changed by someone else/)).toBeVisible();
	await expect(page.locator('[data-room="104"]', { hasText: 'Silva, A.' })).toBeVisible();
	expect(assigns).toBe(1);
});

test('dragging the end of a bar one day right asks to confirm the new total, then extends it', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'RSZ');
	const hotel = await bookableHotel(page, 6, 2);
	await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const bar = page.locator('[data-room="101"]', { hasText: 'Silva, A.' });
	await expect(bar).toBeVisible();
	const box = (await bar.boundingBox())!;
	const day = box.width;
	const from = { x: box.x + box.width - 3, y: box.y + box.height / 2 };

	await drag(page, from, { x: from.x + day, y: from.y });
	const dialog = page.getByRole('dialog', { name: 'Change stay' });
	await expect(dialog).toBeVisible();
	await expect(dialog).toContainText(/USD.*100.*→.*USD.*200/);
	// Nothing is written until it is confirmed; cancelling leaves the bar as it was.
	await dialog.getByRole('button', { name: 'Cancel' }).click();
	await expect(dialog).toBeHidden();
	expect((await bar.boundingBox())!.width).toBeCloseTo(day, 0);

	await drag(page, from, { x: from.x + day, y: from.y });
	await dialog.getByRole('button', { name: 'Confirm' }).click();
	await expect(dialog).toBeHidden();
	await expect(bar).toHaveAttribute(
		'aria-label',
		new RegExp(`to ${addDays(hotel.businessDate, 2)}`)
	);
	expect((await bar.boundingBox())!.width).toBeCloseTo(day * 2, 0);
});

test('the ghost is styled and follows the pointer while a bar is dragged', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GST');
	const hotel = await bookableHotel(page, 6, 1);
	await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const bar = page.locator('[data-room="101"]', { hasText: 'Silva, A.' });
	await expect(bar).toBeVisible();
	const box = (await bar.boundingBox())!;
	const x = box.x + box.width / 2;
	const y = box.y + box.height / 2;

	const ghost = page.locator('.ghost');
	await expect(ghost).toBeHidden();
	await page.mouse.move(x, y);
	await page.mouse.down();
	await page.mouse.move(x, y + 20);
	await page.mouse.move(x, y + 44);
	await expect(ghost).toBeVisible();
	await expect(ghost).toHaveCSS('position', 'absolute');
	await expect(ghost).toHaveCSS('pointer-events', 'none');
	await expect(ghost).toHaveCSS('transform-origin', '0px 0px');
	const dragged = (await ghost.boundingBox())!;
	expect(dragged.y).toBeCloseTo(box.y + 44, 0);
	// Escape drops nothing.
	await page.keyboard.press('Escape');
	await page.mouse.up();
	await expect(ghost).toBeHidden();
	await expect(bar).toBeVisible();
});

async function reservationRoom(page: Page, hotel: Hotel, id: string) {
	const propertyId = hotel.path.split('/').pop();
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ reservation(propertyId: "${propertyId}", id: "${id}") { rooms { total roomType { code } } } }`
		}
	});
	return (await response.json()).data.reservation.rooms[0] as {
		total: number;
		roomType: { code: string };
	};
}

test('dropping a bar on a room of another type can keep the booked price', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'UPG');
	const hotel = await bookableHotel(page, 2, 1);
	const standard = await post(page.request, `${hotel.path}/room-types`, {
		code: 'STD',
		name: 'Standard',
		base_occupancy: 2,
		max_adults: 2,
		max_children: 1,
		max_occupancy: 3
	});
	await post(page.request, `${hotel.path}/rooms/bulk`, {
		room_type_id: standard.id,
		first: 201,
		last: 202
	});
	// The stay's plan sells STD too, at a different price.
	const propertyId = hotel.path.split('/').pop();
	const plans = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { query: `{ ratePlans(propertyId: "${propertyId}") { id version } }` }
	});
	const plan = (await plans.json()).data.ratePlans.find(
		(candidate: { id: string }) => candidate.id === hotel.ratePlanId
	);
	const patched = await page.request.patch(`${hotel.path}/rate-plans/${hotel.ratePlanId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${plan.version}"` },
		data: { room_type_ids: [hotel.roomTypeId, standard.id] }
	});
	expect(patched.status(), await patched.text()).toBe(200);
	const priced = await page.request.put(`${hotel.path}/rate-plans/${hotel.ratePlanId}/prices`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			prices: [
				{ room_type_id: standard.id, date: hotel.businessDate, occupancy: 2, amount: 15_000 }
			]
		}
	});
	expect(priced.status(), await priced.text()).toBe(204);
	const stay = await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toBeVisible();
	await expect((await reservationRoom(page, hotel, stay.id)).total).toBe(10_000);

	const box = (await page.locator('[data-room="101"]', { hasText: 'Silva, A.' }).boundingBox())!;
	const x = box.x + box.width / 2;
	const y = box.y + box.height / 2;
	await drag(page, { x, y }, { x, y: y + 44 * 2 });
	const dialog = page.getByRole('dialog', { name: 'Change stay' });
	await expect(dialog).toContainText(/USD.*100.*→.*USD.*150/);
	await dialog.getByLabel('Keep the booked price (upgrade)').check();
	await dialog.getByRole('button', { name: 'Confirm' }).click();
	await expect(dialog).toBeHidden();

	await expect(page.locator('[data-room="201"]', { hasText: 'Silva, A.' })).toBeVisible();
	await expect(page.locator('[data-room="101"]', { hasText: 'Silva, A.' })).toHaveCount(0);
	await expect
		.poll(async () => reservationRoom(page, hotel, stay.id))
		.toEqual({
			total: 10_000,
			roomType: { code: 'STD' }
		});
});

test('a checked-in stay can only have its departure edge dragged', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'CKI');
	const hotel = await bookableHotel(page, 6, 2);
	const stay = await bookTonight(page, hotel);
	const checkedIn = await page.request.post(
		`${hotel.path}/reservation-rooms/${stay.roomId}/check-in`,
		{
			headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${stay.version}"` }
		}
	);
	expect(checkedIn.status(), await checkedIn.text()).toBe(200);
	await page.getByRole('link', { name: 'Tape chart' }).click();
	const bar = page.locator('[data-room="101"]', { hasText: 'Silva, A.' });
	await expect(bar).toBeVisible();
	await expect(bar.locator('.grip.start')).toHaveCount(0);
	await expect(bar.locator('.grip.end')).toHaveCount(1);
	const box = (await bar.boundingBox())!;
	const y = box.y + box.height / 2;
	const dialog = page.getByRole('dialog', { name: 'Change stay' });

	// The body and the arrival edge go nowhere: no dialog, no move.
	await drag(page, { x: box.x + box.width / 2, y }, { x: box.x + box.width / 2, y: y + 44 });
	await drag(page, { x: box.x + 2, y }, { x: box.x + 2 + box.width, y });
	await expect(dialog).toBeHidden();
	await expect(bar).toHaveAttribute('aria-label', new RegExp(`${hotel.businessDate} to`));

	await drag(page, { x: box.x + box.width - 3, y }, { x: box.x + box.width * 2 - 3, y });
	await expect(dialog).toBeVisible();
	await dialog.getByRole('button', { name: 'Confirm' }).click();
	await expect(dialog).toBeHidden();
	await expect(bar).toHaveAttribute(
		'aria-label',
		new RegExp(`to ${addDays(hotel.businessDate, 2)}`)
	);
});
