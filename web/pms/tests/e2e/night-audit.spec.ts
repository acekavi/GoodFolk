import { expect, test, type Page } from '@playwright/test';
import {
	addDays,
	bookableHotel,
	createProperty,
	post,
	rollBusinessDate,
	signUp,
	unassign,
	type Hotel
} from './helpers';

// The night audit end to end, in a real browser against the real API and jobs-svc (which runs the close):
// resolving the day's arrivals and departures, closing the day and following the run, the history and a
// failed step's retry. Playwright starts jobs-svc beside core-api, with its own ticking off.

/** Rows of a table, by the confirmation number they show. */
const rowOf = (page: Page, confirmation: string) =>
	page.getByRole('row').filter({ hasText: confirmation });

/** The tenant and property of the signed-in user's open property page. */
async function ids(page: Page) {
	const me = await page.request.get('/api/v1/me');
	expect(me.status()).toBe(200);
	return {
		tenant: (await me.json()).current_tenant as string,
		propertyId: page.url().split('/p/')[1].split('/')[0]
	};
}

/** A signed-up owner's property of 3 rooms priced for the next 3 nights, with the page left on it. */
async function hotel(page: Page, code: string, options: { rates?: boolean } = {}) {
	await signUp(page);
	if (options.rates ?? true) {
		await createProperty(page, code);
	} else {
		// The same property, but without exchange rates.
		await page.getByRole('link', { name: 'Add a property' }).click();
		await page.getByLabel('Code').fill(code);
		await page.getByLabel('Name').fill(`Hotel ${code}`);
		await page.getByRole('button', { name: 'Create property' }).click();
		await expect(page.getByRole('heading', { name: `Hotel ${code}` })).toBeVisible();
	}
	return { ...(await ids(page)), ...(await bookableHotel(page, 3, 3)) };
}

/**
 * Books one DLX room from `from` (a night offset from the hotel's business date) for `nights` nights and
 * checks it in if `checkIn`. A stay that is not checked in is left without a room, so that it takes none from
 * the stays the test extends or checks in.
 */
async function bookStay(
	page: Page,
	hotel: Hotel,
	options: { from?: number; nights?: number; checkIn?: boolean } = {}
) {
	const from = options.from ?? 0;
	const created = await post(page.request, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: addDays(hotel.businessDate, from),
				check_out: addDays(hotel.businessDate, from + (options.nights ?? 1)),
				adults: 2
			}
		]
	});
	const room = created.rooms[0] as { id: string; version: number };
	if (options.checkIn) {
		const response = await page.request.post(
			`${hotel.path}/reservation-rooms/${room.id}/check-in`,
			{
				headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${room.version}"` }
			}
		);
		expect(response.status(), await response.text()).toBe(200);
	} else {
		await unassign(page.request, hotel, room.id, room.version);
	}
	return { id: created.id as string, confirmation: created.confirmation_no as string };
}

/** Gives BAR a no-show fee of one night; bookings made afterwards copy it. */
async function chargeNoShows(page: Page, hotel: Hotel) {
	const policy = await post(page.request, `${hotel.path}/cancellation-policies`, {
		name: 'Strict',
		rules: [],
		no_show: { kind: 'nights', value: 1 }
	});
	const planned = await page.request.patch(`${hotel.path}/rate-plans/${hotel.ratePlanId}`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' },
		data: { cancellation_policy_id: policy.id }
	});
	expect(planned.status(), await planned.text()).toBe(200);
}

test('an arrival is marked a no-show, a departure extended, and the day closes into the history', async ({
	page
}) => {
	const owner = await hotel(page, 'NAF');
	await chargeNoShows(page, owner);
	// The departure and the stay in house arrived tonight; the day rolls, so that the first leaves on the
	// business date; and an arrival then comes for it.
	const departure = await bookStay(page, owner, { checkIn: true });
	const inHouse = await bookStay(page, owner, { nights: 2, checkIn: true });
	rollBusinessDate(owner.tenant, owner.propertyId);
	const date = addDays(owner.businessDate, 1);
	const arrival = await bookStay(page, owner, { from: 1, nights: 2 });

	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await expect(page.getByText(`Business date ${date}`)).toBeVisible();
	await expect(page.getByText('2 to resolve')).toBeVisible();
	const close = page.getByRole('button', { name: 'Close the day' });
	await expect(close).toBeDisabled();
	await expect(rowOf(page, inHouse.confirmation)).toHaveCount(0);

	// The arrival: a no-show, with its fee.
	await rowOf(page, arrival.confirmation).getByRole('button', { name: 'No-show' }).click();
	const noShow = page.getByRole('dialog', { name: 'Mark as a no-show' });
	await expect(noShow).toContainText('A no-show fee of USD 100.00 is posted');
	await noShow.getByRole('button', { name: 'Mark no-show' }).click();
	await expect(page.getByRole('status')).toContainText('The fee of USD 100.00 is posted');
	await expect(page.getByText('1 to resolve')).toBeVisible();

	// The departure: another night.
	await rowOf(page, departure.confirmation).getByRole('button', { name: 'Extend a night' }).click();
	const extend = page.getByRole('dialog', { name: 'Extend a night' });
	await extend.getByRole('button', { name: 'Extend a night' }).click();
	await expect(page.getByRole('status')).toContainText(
		`the guest now leaves on ${addDays(date, 1)}`
	);
	await expect(page.getByText('Nothing to resolve')).toBeVisible();

	// The no-show fee is on the reservation's deposit folio.
	await page.goto(`/p/${owner.propertyId}/reservations/${arrival.id}`);
	const modal = page.getByRole('dialog', { name: new RegExp(arrival.confirmation) });
	await modal.getByRole('tab', { name: 'Folios', exact: true }).click();
	await modal.getByRole('tab', { name: 'Deposit', exact: true }).click();
	const deposit = modal.getByRole('region', { name: 'Folio Deposit' });
	await expect(deposit.getByRole('list', { name: 'Balances' })).toContainText('due');
	await expect(deposit.getByRole('table', { name: 'Lines of Deposit' })).toContainText(
		'No-show fee'
	);

	// Close the day: the tracker reaches the end and the business date moves on.
	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await page.getByRole('button', { name: 'Close the day' }).click();
	const tracker = page.getByRole('region', { name: `Closing ${date}` });
	await expect(tracker).toContainText(`Business date is now ${addDays(date, 1)}`);
	await expect(tracker.getByText('Done')).toHaveCount(5);
	await expect(page.getByText(`Business date ${addDays(date, 1)}`)).toBeVisible();

	await page.goto(`/p/${owner.propertyId}`);
	await expect(page.getByText(addDays(date, 1), { exact: true })).toBeVisible();

	await page.goto(`/p/${owner.propertyId}/night-audit/history`);
	const run = page.getByRole('row').filter({ hasText: date });
	await expect(run.first()).toContainText('Manual');
	await expect(run.first()).toContainText('Completed');
	await expect(run.first()).toContainText('5 of 5 steps');
});

test('the close stays disabled until the arrival is resolved', async ({ page }) => {
	const owner = await hotel(page, 'NAB');
	const arrival = await bookStay(page, owner, { nights: 2 });

	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await expect(page.getByText('1 to resolve')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Close the day' })).toBeDisabled();

	await rowOf(page, arrival.confirmation).getByRole('button', { name: 'No-show' }).click();
	await page
		.getByRole('dialog', { name: 'Mark as a no-show' })
		.getByRole('button', { name: 'Mark no-show' })
		.click();
	await expect(page.getByText('Nothing to resolve')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Close the day' })).toBeEnabled();
});

test('delaying an arrival takes it off the list and moves the reservation', async ({ page }) => {
	const owner = await hotel(page, 'NAD');
	const arrival = await bookStay(page, owner, { nights: 2 });

	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await rowOf(page, arrival.confirmation).getByRole('button', { name: 'Delay arrival' }).click();
	const dialog = page.getByRole('dialog', { name: 'Delay arrival' });
	await expect(dialog).toContainText(`The stay becomes`);
	await dialog.getByRole('button', { name: 'Delay arrival' }).click();
	await expect(page.getByRole('status')).toContainText('Arrival delayed');
	await expect(rowOf(page, arrival.confirmation)).toHaveCount(0);
	await expect(page.getByText('Nothing to resolve')).toBeVisible();

	await page.goto(`/p/${owner.propertyId}/reservations/${arrival.id}`);
	const modal = page.getByRole('dialog', { name: new RegExp(arrival.confirmation) });
	const nights = modal.getByRole('table', { name: 'Nights' });
	await expect(nights.getByRole('row')).toHaveCount(2);
	await expect(nights).toContainText(addDays(owner.businessDate, 1));
	await expect(nights).not.toContainText(owner.businessDate);
});

test('a stay without an exchange rate fails the close, which completes once the rate is added and retried', async ({
	page
}) => {
	const owner = await hotel(page, 'NAR', { rates: false });
	await bookStay(page, owner, { checkIn: true });

	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await expect(page.getByText('Nothing to resolve')).toBeVisible();
	await page.getByRole('button', { name: 'Close the day' }).click();
	const tracker = page.getByRole('region', { name: `Closing ${owner.businessDate}` });
	await expect(tracker.getByText('Failed', { exact: true })).toBeVisible();
	await expect(tracker.getByRole('cell', { name: '101', exact: true })).toBeVisible();
	await expect(tracker.getByRole('button', { name: 'Retry' })).toBeVisible();

	await page.goto(`/p/${owner.propertyId}/settings/exchange-rates`);
	const rate = page.getByRole('form', { name: 'Set a rate' });
	await rate.getByLabel('Currency').selectOption('USD');
	await rate.getByLabel('Rate').fill('300');
	await rate.getByRole('button', { name: 'Set rate' }).click();
	await expect(page.getByRole('status').filter({ hasText: 'is now 300' })).toBeVisible();

	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await page
		.getByRole('region', { name: `Closing ${owner.businessDate}` })
		.getByRole('button', { name: 'Retry' })
		.click();
	await expect(
		page
			.getByRole('region', { name: `Closing ${owner.businessDate}` })
			.getByText(`Business date is now ${addDays(owner.businessDate, 1)}`)
	).toBeVisible();
});

test('a POS bill dated a closed day is listed today with its own date', async ({ page }) => {
	const owner = await hotel(page, 'NAL');
	await post(page.request, `${owner.path}/outlets`, { code: 'BAR', name: 'Bar' });
	rollBusinessDate(owner.tenant, owner.propertyId);
	await post(page.request, `${owner.path}/pos-bills`, {
		outlet: 'BAR',
		bill_no: 'L-1',
		business_date: owner.businessDate,
		currency: 'LKR',
		items: [{ description: 'Late drinks', quantity: 1, unit_net: 100_000 }],
		target: { settled: { method: 'cash' } }
	});

	await page.goto(`/p/${owner.propertyId}/night-audit`);
	await expect(page.getByText(`Business date ${addDays(owner.businessDate, 1)}`)).toBeVisible();
	const bill = page.getByRole('row').filter({ hasText: 'L-1' });
	await expect(bill).toContainText(`dated ${owner.businessDate}`);
	await expect(bill).toContainText('Settled at the outlet');
});
