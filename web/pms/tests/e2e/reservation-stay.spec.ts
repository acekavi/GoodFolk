import { expect, test } from '@playwright/test';
import { addDays, bookableHotel, createProperty, post, signUp, type Hotel } from './helpers';

/** Books one DLX room on BAR for `nights` nights from the business date, two adults. */
async function bookNights(api: Parameters<typeof post>[0], hotel: Hotel, nights: number) {
	const created = await post(api, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: hotel.businessDate,
				check_out: addDays(hotel.businessDate, nights),
				adults: 2
			}
		]
	});
	return created.id as string;
}

test('the detail modal modifies, checks in and out, manages occupants and bills an account', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'STY');
	// Six DLX rooms: five reservations below each hold one on the business date's night.
	const hotel = await bookableHotel(page, 6, 5);
	const api = page.request;
	const propertyId = hotel.path.split('/').pop();

	// A second room type, on the same plan, for the upgrade test.
	const sup = await post(api, `${hotel.path}/room-types`, {
		code: 'SUP',
		name: 'Superior',
		base_occupancy: 2,
		max_adults: 2,
		max_children: 1,
		max_occupancy: 3
	});
	await post(api, `${hotel.path}/rooms/bulk`, { room_type_id: sup.id, first: 201, last: 201 });
	const planPatched = await api.patch(`${hotel.path}/rate-plans/${hotel.ratePlanId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' },
		data: { room_type_ids: [hotel.roomTypeId, sup.id] }
	});
	expect(planPatched.status(), await planPatched.text()).toBe(200);
	const supPrices = Array.from({ length: 5 }, (_, night) => ({
		room_type_id: sup.id,
		date: addDays(hotel.businessDate, night),
		occupancy: 2,
		amount: 15_000
	}));
	const supPriced = await api.put(`${hotel.path}/rate-plans/${hotel.ratePlanId}/prices`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { prices: supPrices }
	});
	expect(supPriced.status(), await supPriced.text()).toBe(204);

	// A second guest, for the occupants test, and an account, for the billing test.
	await post(api, `${hotel.path}/guests`, {
		first_name: 'Grace',
		last_name: 'Hopper',
		residency: 'non_resident'
	});
	const account = await post(api, `${hotel.path}/accounts`, {
		kind: 'company',
		name: 'Acme Corp',
		currency: 'USD'
	});

	// Four reservations for the four stay scenarios, and a fifth for occupants and the account.
	const extendId = await bookNights(api, hotel, 1);
	const upgradeId = await bookNights(api, hotel, 2);
	const stayId = await bookNights(api, hotel, 3);
	const undoId = await bookNights(api, hotel, 1);
	const otherId = await bookNights(api, hotel, 1);

	const dialog = page.getByRole('dialog');

	// 1. Modify: extend by one night and see the total change.
	await page.goto(`/p/${propertyId}/reservations/${extendId}`);
	let room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('USD 100.00');
	await expect(room.getByRole('table', { name: 'Nights' }).getByRole('row')).toHaveCount(2);
	await room.getByRole('button', { name: 'Modify…' }).click();
	const save = room.getByRole('button', { name: 'Save', exact: true });
	await expect(save).toBeDisabled();
	await room.getByLabel('Check-out').fill(addDays(hotel.businessDate, 2));
	await expect(save).toBeEnabled();
	await save.click();
	await expect(room.getByRole('table', { name: 'Nights' }).getByRole('row')).toHaveCount(3);
	await expect(room).toContainText('USD 200.00');
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 2. An upgrade with keep price moves the type and keeps the total.
	await page.goto(`/p/${propertyId}/reservations/${upgradeId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('USD 200.00');
	await room.getByRole('button', { name: 'Modify…' }).click();
	await room.getByLabel('Room type').selectOption({ label: 'SUP · Superior' });
	await room.getByLabel('Keep the booked price (upgrade)').check();
	await room.getByRole('button', { name: 'Save', exact: true }).click();
	const upgraded = dialog.getByRole('region', { name: 'SUP · Unassigned' });
	await expect(upgraded).toBeVisible();
	await expect(upgraded).toContainText('USD 200.00');
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 3. Assign, then check in (the stay must start today).
	await page.goto(`/p/${propertyId}/reservations/${stayId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await room.getByRole('button', { name: 'Assign room' }).click();
	await room.getByRole('combobox', { name: 'Room' }).selectOption('101');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	room = dialog.getByRole('region', { name: 'DLX · 101' });
	await room.getByRole('button', { name: 'Check in' }).click();
	await expect(room).toContainText('Checked in');

	// 4. Check out early and see the released nights and the Checked out status.
	const released = [addDays(hotel.businessDate, 1), addDays(hotel.businessDate, 2)];
	await room.getByRole('button', { name: 'Check out…' }).click();
	await expect(room).toContainText(
		`Checking out today releases 2 nights (${released.join(', ')}).`
	);
	await room.getByRole('button', { name: 'Check out', exact: true }).click();
	await expect(room).toContainText(`Checked out. Released 2 nights (${released.join(', ')}).`);
	await expect(page.getByRole('dialog', { name: /Checked out/ })).toBeVisible();
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 5. Undo check-in on the same day.
	await page.goto(`/p/${propertyId}/reservations/${undoId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await room.getByRole('button', { name: 'Assign room' }).click();
	await room.getByRole('combobox', { name: 'Room' }).selectOption('102');
	await room.getByRole('button', { name: 'Assign', exact: true }).click();
	room = dialog.getByRole('region', { name: 'DLX · 102' });
	await room.getByRole('button', { name: 'Check in' }).click();
	await expect(room).toContainText('Checked in');
	await room.getByRole('button', { name: 'Undo check-in' }).click();
	await expect(room).toContainText('Confirmed');
	await expect(room.getByRole('button', { name: 'Check in' })).toBeVisible();
	await dialog.getByRole('button', { name: 'Close' }).click();

	// 6. Add and remove an occupant, and 7. set an account.
	await page.goto(`/p/${propertyId}/reservations/${otherId}`);
	room = dialog.getByRole('region', { name: 'DLX · Unassigned' });
	await expect(room).toContainText('No additional occupants.');
	await room.getByRole('button', { name: 'Add occupant…' }).click();
	await room.getByLabel('Find a guest').fill('Grace');
	await room.getByRole('button', { name: 'Grace Hopper' }).click();
	const occupant = room.getByRole('listitem').filter({ hasText: 'Grace Hopper' });
	await expect(occupant).toBeVisible();
	await occupant.getByRole('button', { name: 'Remove' }).click();
	await expect(room).toContainText('No additional occupants.');

	await dialog.getByLabel('Billed account').selectOption({ label: 'Acme Corp · Company' });
	await expect(dialog.getByLabel('Billed account')).toHaveValue(account.id);
	await page.reload();
	await expect(dialog.getByLabel('Billed account')).toHaveValue(account.id);
});
