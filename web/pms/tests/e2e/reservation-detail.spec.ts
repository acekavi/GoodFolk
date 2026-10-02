import { expect, test, type Page } from '@playwright/test';
import {
	addDays,
	bookableHotel,
	createProperty,
	post,
	signUp,
	unassign,
	type Hotel
} from './helpers';

const ID_NUMBER = 'P98765432';

/**
 * Books one DLX room on BAR for the business date's night, takes it out of the room booking auto-assigned it
 * (the flow below assigns by hand), and returns the reservation and its room.
 */
async function bookOneNight(api: Parameters<typeof post>[0], hotel: Hotel) {
	const created = await post(api, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'phone',
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
		confirmation: created.confirmation_no as string,
		roomId: created.rooms[0].id as string,
		roomVersion: await unassign(api, hotel, created.rooms[0].id, created.rooms[0].version)
	};
}

test('a reservation opens in a modal over the table, where rooms are assigned, unassigned and cancelled with the penalty shown first', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'DET');
	const hotel = await bookableHotel(page, 3, 1);
	const api = page.request;

	// BAR charges a night for cancelling within a week of arrival; bookings copy the terms.
	const policy = await post(api, `${hotel.path}/cancellation-policies`, {
		name: 'Week',
		rules: [{ days_before_arrival: 7, penalty: { kind: 'nights', value: 1 } }],
		no_show: { kind: 'nights', value: 1 }
	});
	const planned = await api.patch(`${hotel.path}/rate-plans/${hotel.ratePlanId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' },
		data: { cancellation_policy_id: policy.id }
	});
	expect(planned.status(), await planned.text()).toBe(200);
	const guest = await post(api, `${hotel.path}/guests`, {
		first_name: 'Grace',
		last_name: 'Hopper',
		residency: 'non_resident',
		id_doc: { type: 'passport', number: ID_NUMBER }
	});
	const booked = { ...hotel, guestId: guest.id };
	const first = await bookOneNight(api, booked);
	const second = await bookOneNight(api, booked);
	expect(first.confirmation).toBe('DET-000001');
	expect(second.confirmation).toBe('DET-000002');

	// The table, filtered, then the second reservation's modal over it.
	await page.getByRole('link', { name: 'Reservations' }).click();
	await page.getByLabel('Search').fill('det-00000');
	await expect(page).toHaveURL(/[?&]text=det-00000(&|$)/);
	const table = page.getByRole('table', { name: 'Reservations' });
	const row = (confirmation: string) =>
		table.getByRole('row').filter({ has: page.getByRole('link', { name: confirmation }) });
	await row('DET-000002').getByRole('link').click();
	// Named by its confirmation number and status, which changes as the rooms do.
	const dialog = page.getByRole('dialog');
	await expect(page.getByRole('dialog', { name: 'DET-000002 · Confirmed' })).toBeVisible();
	await expect(page).toHaveURL(new RegExp(`/reservations/${second.id}\\?text=det-00000$`));
	await expect(dialog.getByRole('button', { name: 'Close' })).toBeFocused();

	// Stay, booker (masked ID), the room, its nights, terms and history.
	await expect(dialog).toContainText('Phone');
	await expect(dialog).toContainText('Grace Hopper');
	await expect(dialog).toContainText('Passport •••• 5432');
	const room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('2 adults');
	await expect(room).toContainText('BAR · Room only');
	await expect(room).toContainText('Free until 7 days before arrival, then 1 night');
	const nights = room.getByRole('table', { name: 'Nights' });
	await expect(nights.getByRole('row')).toHaveCount(2);
	await expect(nights).toContainText(hotel.businessDate);
	await expect(nights).toContainText('100.00');
	await expect(room).toContainText('USD 100.00');
	const history = dialog.getByRole('table', { name: 'History' });
	await expect(history).toContainText('Booked');
	await expect(history).toContainText('Nimal Perera');

	// The picker lists the free rooms; meanwhile room 101 goes to the first reservation, so picking it is
	// refused with the server's reason and the picker no longer offers it. The picker's list is held as it
	// was when it opened (any refetch, such as the resync when the event stream connects, gets the same
	// answer) until 101 is picked, as for someone who opened it a moment before the other booking.
	let offered: string | undefined;
	await page.route('**/graphql', async (route) => {
		if (!(route.request().postData() ?? '').includes('FreeRooms(')) return route.fallback();
		offered ??= await (await route.fetch()).text();
		await route.fulfill({ contentType: 'application/json', body: offered });
	});
	await room.getByRole('button', { name: 'Assign room' }).click();
	const picker = room.getByRole('combobox', { name: 'Room' });
	await expect(picker.getByRole('option')).toHaveText(['Choose a room', '101', '102', '103']);
	const assigned = await api.post(`${hotel.path}/reservation-rooms/${first.roomId}/assign`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${first.roomVersion}"` },
		data: { room_id: await roomId(page, hotel, '101') }
	});
	expect(assigned.status(), await assigned.text()).toBe(200);
	await picker.selectOption('101');
	await page.unroute('**/graphql');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	await expect(room.getByRole('alert')).toHaveText(
		'room 101 is taken by DET-000001 on those nights'
	);
	// The refused room drops out, but the picker never silently switches to another one: the user is back
	// to choosing, not looking at a room they never picked.
	await expect(picker.getByRole('option')).toHaveText(['Choose a room', '102', '103']);
	await expect(picker).toHaveValue('');
	await expect(room.getByRole('button', { name: 'Assign', exact: true })).toBeDisabled();

	// Assigning a free room shows in the modal and in the table's row.
	await picker.selectOption('102');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	const assignedRoom = dialog.getByRole('region', { name: 'DLX · 102' });
	await expect(assignedRoom).toBeVisible();
	await expect(row('DET-000002')).toContainText('DLX · 102');
	await expect(history).toContainText('Room 102 assigned');

	// Unassigning puts it back.
	await assignedRoom.getByRole('button', { name: 'Unassign' }).click();
	const unassigned = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(unassigned).toBeVisible();
	await expect(row('DET-000002')).toContainText('DLX · unassigned');
	await expect(history).toContainText('Room 102 unassigned');

	// Cancelling shows what it costs before it is done, then what was recorded.
	await unassigned.getByRole('button', { name: 'Cancel room…' }).click();
	await expect(unassigned).toContainText('Cancelling now costs USD 100.00');
	await unassigned.getByRole('button', { name: 'Cancel this room' }).click();
	const cancelled = page.getByRole('dialog', { name: 'DET-000002 · Cancelled' });
	await expect(cancelled).toBeVisible();
	await expect(unassigned.getByRole('status')).toHaveText(
		'Cancelled. The recorded penalty is USD 100.00.'
	);
	await expect(unassigned).toContainText('Cancellation cost USD 100.00');
	await expect(unassigned.getByRole('button', { name: 'Cancel room…' })).toHaveCount(0);
	await expect(unassigned.getByRole('button', { name: 'Assign room' })).toHaveCount(0);
	await expect(row('DET-000002')).toContainText('Cancelled');
	await expect(history).toContainText('Room cancelled, costing USD 100.00');

	// Escape closes it: back to the list with its filter, focus on the row that opened it.
	await page.keyboard.press('Escape');
	await expect(cancelled).toBeHidden();
	await expect(page).toHaveURL(/\/reservations\?text=det-00000$/);
	await expect(page.getByLabel('Search')).toHaveValue('det-00000');
	await expect(row('DET-000002').getByRole('link')).toBeFocused();

	// A deep link on a fresh page opens the modal; closing it goes to the list.
	await page.goto(`/p/${hotel.path.split('/').pop()}/reservations/${first.id}`);
	const deepLinked = page.getByRole('dialog', { name: 'DET-000001 · Confirmed' });
	await expect(deepLinked).toBeVisible();
	await expect(deepLinked.getByRole('region', { name: 'DLX · 101' })).toBeVisible();
	await expect(deepLinked).toContainText('•••• 5432');
	const content = await page.content();
	expect(content).not.toContain(ID_NUMBER);
	expect(content).not.toContain('98765432');
	expect(content).not.toContain('9876');
	await deepLinked.getByRole('button', { name: 'Close' }).click();
	await expect(deepLinked).toBeHidden();
	await expect(page).toHaveURL(/\/reservations$/);
	await expect(row('DET-000001')).toContainText('DLX · 101');

	// A click on the backdrop closes it too.
	await row('DET-000001').getByRole('link').click();
	await expect(deepLinked).toBeVisible();
	await page.mouse.click(5, 5);
	await expect(deepLinked).toBeHidden();
	await expect(page).toHaveURL(/\/reservations$/);
	await expect(row('DET-000001').getByRole('link')).toBeFocused();
});

/** The id of room `number`, through GraphQL. */
async function roomId(page: Page, hotel: Hotel, number: string) {
	const propertyId = hotel.path.split('/').pop();
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: 'query ($p: UUID!) { rooms(propertyId: $p) { id number } }',
			variables: { p: propertyId }
		}
	});
	const { data } = await response.json();
	return data.rooms.find((room: { number: string }) => room.number === number).id as string;
}
