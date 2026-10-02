import { expect, test, type Page } from '@playwright/test';
import {
	addDays,
	bookableHotel,
	createProperty,
	PASSWORD,
	post,
	runsQuery,
	signUp,
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

test('a booking is a bar in its room that opens the reservation, and Escape comes back to the chart', async ({
	page
}) => {
	const hotel = await twelveRooms(page);
	const { id } = await bookTonight(page, hotel);
	await page.getByRole('link', { name: 'Tape chart' }).click();

	const bar = page.locator('[data-room="101"]', { hasText: 'Silva, A.' });
	await expect(bar).toBeVisible();
	await bar.click();
	await expect(page).toHaveURL(new RegExp(`/reservations/${id}`));
	await expect(page.getByRole('dialog')).toBeVisible();
	await page.keyboard.press('Escape');
	await expect(page).toHaveURL(/\/tape(\?|$)/);
	await expect(bar).toBeVisible();
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
