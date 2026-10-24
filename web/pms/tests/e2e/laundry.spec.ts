import { expect, test, type Page } from '@playwright/test';
import {
	addStaff,
	bookableHotel,
	createProperty,
	post,
	signIn,
	signUp,
	addDays,
	type Hotel
} from './helpers';

// Linen and guest laundry end to end, in a real browser against the real API: a linen batch received short and
// written off, a guest laundry order taken from the reservation modal and delivered to the folio, and what a
// housekeeping user may do. Prices and tax come from the property's base currency (LKR) and the folio's rules.

/** A signed-up owner's bookable property, with the page left on it. */
async function hotel(page: Page, code: string) {
	const { email } = await signUp(page);
	await createProperty(page, code);
	const propertyId = page.url().split('/p/')[1];
	return { email, propertyId, ...(await bookableHotel(page, 2, 1)) };
}

/** Sets up the store, the in-house laundry and two items on the Linen page's Setup tab. */
async function setUpLinen(page: Page, propertyId: string) {
	await page.goto(`/p/${propertyId}/linen`);
	await page.getByRole('tab', { name: 'Setup' }).click();

	const items = page.getByRole('form', { name: 'New linen item' });
	for (const [code, name] of [
		['TOWEL', 'Bath towel'],
		['SHEET', 'Bed sheet']
	]) {
		await items.getByLabel('Code').fill(code);
		await items.getByLabel('Name').fill(name);
		await items.getByRole('button', { name: 'Add item' }).click();
		await expect(page.getByRole('table', { name: 'Linen items' })).toContainText(name);
	}

	const locations = page.getByRole('form', { name: 'New linen location' });
	for (const [name, kind] of [
		['Main store', 'Store'],
		['In-house laundry', 'Laundry']
	]) {
		await locations.getByLabel('Name').fill(name);
		await locations.getByLabel('Kind').selectOption({ label: kind });
		await locations.getByRole('button', { name: 'Add location' }).click();
		await expect(page.getByRole('table', { name: 'Linen locations' })).toContainText(name);
	}

	await page.getByRole('button', { name: 'Edit par for Deluxe' }).click();
	await page.getByLabel('Bath towel per Deluxe room').fill('2');
	await page.getByLabel('Bed sheet per Deluxe room').fill('1');
	await page.getByRole('button', { name: 'Save par for Deluxe' }).click();
	await expect(page.getByRole('button', { name: 'Edit par for Deluxe' })).toBeVisible();
	await expect(
		page.getByRole('table', { name: 'Par per room' }).getByRole('row', { name: /Deluxe/ })
	).toContainText('2');
}

/** Adds a laundry price on Settings → Laundry prices. */
async function addPrice(page: Page, propertyId: string, code: string, name: string, price: string) {
	await page.goto(`/p/${propertyId}/settings/laundry-prices`);
	const form = page.getByRole('form', { name: 'New laundry price' });
	await form.getByLabel('Item code').fill(code);
	await form.getByLabel('Service').selectOption({ label: 'Wash' });
	await form.getByLabel('Name').fill(name);
	await form.getByLabel(/^Price/).fill(price);
	await form.getByRole('button', { name: 'Add price' }).click();
	await expect(page.getByRole('table', { name: 'Laundry prices' })).toContainText(name);
}

/** Books one DLX room for the business date's night and checks the stay in. */
async function checkedInStay(page: Page, hotel: Hotel) {
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
	const room = created.rooms[0] as { id: string; version: number };
	const checkedIn = await page.request.post(`${hotel.path}/reservation-rooms/${room.id}/check-in`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${room.version}"` }
	});
	expect(checkedIn.status(), await checkedIn.text()).toBe(200);
	return { id: created.id as string, confirmation: created.confirmation_no as string };
}

test('a linen batch is received short, then the missing items are written off', async ({
	page
}) => {
	const owner = await hotel(page, 'LIN');
	await setUpLinen(page, owner.propertyId);

	// Buy in 10 towels to the store.
	await page.getByRole('tab', { name: 'Stock' }).click();
	await page.getByRole('button', { name: 'Move linen' }).click();
	const move = page.getByRole('dialog', { name: 'Move linen' });
	await move.getByLabel('Item').selectOption({ label: 'Bath towel (TOWEL)' });
	await move
		.getByRole('combobox', { name: 'To', exact: true })
		.selectOption({ label: 'Main store' });
	await move.getByLabel('Quantity').fill('10');
	await move.getByRole('button', { name: 'Record' }).click();
	await expect(move).toBeHidden();
	const stock = page.getByRole('table', { name: 'Stock' });
	await expect(stock.getByRole('row', { name: /Bath towel/ })).toContainText('10');

	// Send 6 to the laundry.
	await page.getByRole('tab', { name: 'Batches' }).click();
	const send = page.getByRole('form', { name: 'Send a batch' });
	await send.getByRole('combobox', { name: 'From' }).selectOption({ label: 'Main store' });
	await send.getByRole('combobox', { name: 'To' }).selectOption({ label: 'In-house laundry' });
	await send.getByLabel('Bath towel to send').fill('6');
	await send.getByRole('button', { name: 'Send' }).click();
	await expect(page.getByRole('status').filter({ hasText: 'Batch sent.' })).toBeVisible();
	const batches = page.getByRole('table', { name: 'Batches' });
	await expect(batches).toContainText('Bath towel: 6 sent');

	// Four come back: two are short.
	await batches.getByRole('button', { name: 'Receive' }).click();
	const receive = page.getByRole('dialog', { name: 'Receive a batch' });
	await receive.getByLabel('Bath towel count').fill('4');
	await expect(receive.getByRole('row', { name: /Bath towel/ })).toContainText('-2');
	await expect(receive.getByRole('status')).toContainText('2 short');
	await receive.getByLabel('Back to').selectOption({ label: 'Main store' });
	await receive.getByRole('button', { name: 'Confirm receipt' }).click();
	await expect(receive).toBeHidden();
	await expect(batches).toContainText('Bath towel: 6 sent, 4 back, 2 out');

	// The manager writes the two off.
	await batches.getByRole('button', { name: 'Write off' }).click();
	const writeOff = page.getByRole('dialog', { name: 'Write off outstanding items' });
	await writeOff.getByLabel('Reason').fill('Lost at the laundry');
	await writeOff.getByRole('button', { name: 'Write off' }).click();
	await expect(writeOff).toBeHidden();
	await expect(batches).toContainText('4 back, 2 written off');
	await expect(batches.getByRole('button', { name: 'Write off' })).toHaveCount(0);

	// The ledger has it all.
	await page.getByRole('tab', { name: 'Movements' }).click();
	const movements = page.getByRole('table', { name: 'Movements' });
	await expect(movements).toContainText('Lost at the laundry');
	await expect(movements.getByRole('row', { name: /Lost at the laundry/ })).toContainText('2');
});

test('a guest laundry order is taken, delivered and posted to the stay folio with its taxes', async ({
	page
}) => {
	const owner = await hotel(page, 'GST');
	await addPrice(page, owner.propertyId, 'SHIRT', 'Shirt', '500');
	const reservation = await checkedInStay(page, owner);

	await page.goto(`/p/${owner.propertyId}/reservations/${reservation.id}`);
	const modal = page.getByRole('dialog', { name: new RegExp(reservation.confirmation) });
	await modal.getByRole('button', { name: 'Laundry', exact: true }).click();
	const form = page.getByRole('dialog', { name: /Laundry order/ });
	await form.getByLabel('Shirt Wash quantity').fill('2');
	await expect(form.getByRole('status')).toContainText('1,000.00');
	await form.getByRole('button', { name: 'Take order' }).click();
	await expect(form).toBeHidden();
	await expect(modal.getByRole('list', { name: 'Laundry orders' })).toContainText('Shirt');

	page.on('dialog', (dialog) => void dialog.accept());
	await page.goto(`/p/${owner.propertyId}/laundry`);
	const card = page.getByRole('article', { name: 'Room 101 laundry' });
	await expect(card).toContainText('Shirt (Wash) × 2');
	await card.getByRole('button', { name: 'Start' }).click();
	await card.getByRole('button', { name: 'Ready' }).click();
	await expect(
		page.getByRole('region', { name: /Ready/ }).getByRole('article', { name: 'Room 101 laundry' })
	).toBeVisible();
	await card.getByRole('button', { name: 'Deliver' }).click();
	await expect(
		page
			.getByRole('region', { name: /Delivered today/ })
			.getByRole('article', { name: 'Room 101 laundry' })
	).toBeVisible();

	await page.goto(`/p/${owner.propertyId}/reservations/${reservation.id}`);
	const stay = page.getByRole('dialog', { name: new RegExp(reservation.confirmation) });
	await stay.getByRole('tab', { name: 'Folios', exact: true }).click();
	await stay.getByRole('tab', { name: 'Room 101', exact: true }).click();
	const lines = stay.getByRole('table', { name: 'Lines of Room 101' });
	const line = lines.getByRole('row', { name: /Laundry: Shirt × 2/ });
	await expect(line).toContainText('LKR 1,035.25');
	await line.getByRole('button', { name: 'Taxes' }).click();
	const taxes = lines.locator('dl.taxes');
	await expect(taxes).toContainText('Net');
	await expect(taxes).toContainText('LKR 1,000.00');
	await expect(taxes).toContainText('Tourism development levy 1%');
	await expect(taxes).toContainText('LKR 10.00');
});

test('housekeeping sees the Linen page but cannot write off', async ({ page, browser }) => {
	const owner = await hotel(page, 'ACC');
	await setUpLinen(page, owner.propertyId);
	const { email } = addStaff(owner.email, 'housekeeping', 'Kumari Silva');

	const context = await browser.newContext();
	const staff = await context.newPage();
	await signIn(staff, email);
	await staff.goto(`/p/${owner.propertyId}/linen`);
	await expect(staff.getByRole('heading', { name: 'Linen' })).toBeVisible();
	await expect(staff.getByRole('link', { name: 'Linen', exact: true })).toBeVisible();
	await expect(staff.getByRole('tab', { name: 'Setup' })).toHaveCount(0);

	await staff.getByRole('button', { name: 'Move linen' }).click();
	const move = staff.getByRole('dialog', { name: 'Move linen' });
	await expect(move.getByLabel('Kind').getByRole('option')).toHaveText([
		'Receive (bought in)',
		'Transfer'
	]);
	await context.close();
});
