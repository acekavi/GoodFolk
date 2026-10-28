import { expect, test, type Browser, type Page } from '@playwright/test';
import {
	addDays,
	addStaff,
	bookableHotel,
	createProperty,
	post,
	runAsOwner,
	signIn,
	signUp,
	type Hotel
} from './helpers';

// The reports end to end, in a real browser against the real API, jobs-svc (which closes the days) and media-svc
// (which renders the PDFs): the everyday report, the daily lists, the liability reports with their downloads, and
// the report emails. The property is moved back in time so that days of last week are closed.

/** The Monday of the week `date` is in. */
function mondayOf(date: string): string {
	const weekday = (new Date(`${date}T00:00:00Z`).getUTCDay() + 6) % 7;
	return addDays(date, -weekday);
}

/** The business date of the property, as the API gives it. */
async function businessDate(page: Page, propertyId: string): Promise<string> {
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { query: '{ properties { id businessDate } }' }
	});
	const { data } = await response.json();
	return data.properties.find((property: { id: string }) => property.id === propertyId)
		.businessDate;
}

/** Closes the property's business date and waits for the next one. */
async function closeDay(page: Page, owner: { propertyId: string }) {
	const date = await businessDate(page, owner.propertyId);
	const response = await page.request.post(
		`/api/v1/properties/${owner.propertyId}/night-audit/close`,
		{ headers: { 'x-goodfolk-csrf': '1' } }
	);
	expect(response.status(), await response.text()).toBeLessThan(300);
	await expect
		.poll(() => businessDate(page, owner.propertyId), { timeout: 60_000 })
		.toBe(addDays(date, 1));
}

/**
 * A signed-up owner's property whose Friday, Saturday and Sunday of last week are closed, with one stay of four
 * nights in house from the Friday. The business date is this week's Monday, so that last week is a week of
 * which only three days are closed.
 */
async function closedWeek(page: Page, code: string) {
	const { email } = await signUp(page);
	await createProperty(page, code);
	const propertyId = page.url().split('/p/')[1];
	const me = await page.request.get('/api/v1/me');
	const tenant: string = (await me.json()).current_tenant;
	const monday = mondayOf(await businessDate(page, propertyId));
	const friday = addDays(monday, -3);
	// Moves the business date back to the Friday before anything is booked.
	runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			await tx\`update property set business_date = \${process.env.FRIDAY}::date where id = \${process.env.PROPERTY}\`;
		});
		await sql.close();
	`,
		{ TENANT: tenant, PROPERTY: propertyId, FRIDAY: friday }
	);
	const hotel: Hotel = await bookableHotel(page, 3, 4);
	const reservation = await post(page.request, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: hotel.businessDate,
				check_out: addDays(hotel.businessDate, 4),
				adults: 2
			}
		]
	});
	const room = reservation.rooms[0] as { id: string; version: number };
	const checkedIn = await page.request.post(`${hotel.path}/reservation-rooms/${room.id}/check-in`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${room.version}"` }
	});
	expect(checkedIn.status(), await checkedIn.text()).toBe(200);
	for (let day = 0; day < 3; day++) await closeDay(page, { propertyId });
	return { email, propertyId, hotel, monday, friday, confirmation: reservation.confirmation_no };
}

/** A page of `email`, signed in, in a context of its own. */
async function signedIn(browser: Browser, email: string) {
	const context = await browser.newContext();
	const page = await context.newPage();
	await signIn(page, email);
	return { page, context };
}

/** Clicks `button` and returns the first bytes of the PDF whose link the report status gives. */
async function openedPdf(page: Page, button: ReturnType<Page['getByRole']>) {
	const ready = page.waitForResponse(async (response) => {
		if (!/\/reports\/[0-9a-f-]{36}$/.test(response.url()) || !response.ok()) return false;
		return !!(await response.json()).url;
	});
	await button.click();
	const { url } = await (await ready).json();
	const response = await page.request.get(url);
	expect(response.status()).toBe(200);
	return (await response.body()).subarray(0, 4).toString('latin1');
}

test('a manager reads last week, with the days not closed yet, and downloads its PDF', async ({
	page,
	browser
}) => {
	const owner = await closedWeek(page, 'RPW');
	const { email } = addStaff(owner.email, 'manager', 'Mala Perera');
	const { page: manager, context } = await signedIn(browser, email);

	await manager.goto(`/p/${owner.propertyId}`);
	await manager.getByRole('link', { name: 'Reports', exact: true }).click();
	await manager.getByRole('button', { name: 'Last week' }).click();
	await expect(manager.getByText('Not closed yet: figures cover')).toContainText(
		`${owner.friday}–${addDays(owner.friday, 2)}`
	);
	const headline = manager.getByRole('region', { name: 'Headline figures' });
	await expect(headline).toContainText('Occupancy');
	await expect(headline).toContainText('33.3%');
	await expect(headline).toContainText('Room nights sold');
	const days = manager.getByRole('region', { name: 'Days' });
	await expect(days.getByRole('row').filter({ hasText: owner.friday })).toContainText('33.3%');
	await expect(days.getByRole('row').filter({ hasText: addDays(owner.monday, -7) })).toContainText(
		'Not closed yet'
	);
	await expect(manager.getByRole('table', { name: 'Revenue by room type' })).toContainText('DLX');

	expect(await openedPdf(manager, manager.getByRole('button', { name: 'Download PDF' }))).toBe(
		'%PDF'
	);
	await context.close();
});

test('an accountant opens the TDL Form A of the quarter and downloads it', async ({
	page,
	browser
}) => {
	const owner = await closedWeek(page, 'RPT');
	const { email } = addStaff(owner.email, 'accountant', 'Ann Accountant');
	const { page: accountant, context } = await signedIn(browser, email);

	await accountant.goto(`/p/${owner.propertyId}`);
	await accountant.getByRole('link', { name: 'Tax reports' }).click();
	await expect(accountant.getByRole('link', { name: 'Daily lists' })).toBeVisible();
	await accountant.getByRole('tab', { name: 'TDL' }).click();
	const periods = accountant.getByRole('region', { name: 'Periods' });
	await periods.getByRole('button').nth(0).click();
	const form = accountant.getByRole('table', { name: 'Form A' });
	for (const row of [
		'Room Sales',
		'Food Sales',
		'Bar Sales',
		'Other Sales',
		'Total Turnover',
		'Deduction: Service Charge',
		'Deduction: VAT',
		'Total Turnover less deductions',
		'TDL at'
	]) {
		await expect(form.getByRole('row').filter({ hasText: row }).first()).toBeVisible();
	}
	await expect(form.getByRole('row').filter({ hasText: 'Room Sales' })).toContainText(/Rs [1-9]/);

	expect(
		await openedPdf(accountant, accountant.getByRole('button', { name: 'Download PDF' }))
	).toBe('%PDF');

	// Not VAT registered: the server's refusal is shown.
	await accountant.getByRole('tab', { name: 'VAT' }).click();
	await periods.getByRole('button').nth(0).click();
	await expect(accountant.getByRole('alert')).toContainText('VAT');
	await context.close();
});

test('a VAT registered property with a TIN downloads Schedule 01 with its header row', async ({
	page
}) => {
	const owner = await closedWeek(page, 'RPV');
	const settings = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ propertyTaxSettings(propertyId: "${owner.propertyId}") { version } }`
		}
	});
	const version = (await settings.json()).data.propertyTaxSettings.version;
	const registered = await page.request.patch(`${owner.hotel.path}/tax-settings`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${version}"` },
		data: { vat_registered: true, tin: '134012345' }
	});
	expect(registered.status(), await registered.text()).toBe(200);

	await page.goto(`/p/${owner.propertyId}/reports/tax`);
	await page.getByRole('tab', { name: 'VAT' }).click();
	await page.getByRole('region', { name: 'Periods' }).getByRole('button').nth(0).click();
	await expect(page.getByRole('table', { name: 'VAT return' })).toBeVisible();
	await expect(page.getByRole('table', { name: 'Schedule 01' })).toContainText("Purchaser's TIN");

	const download = page.waitForEvent('download');
	await page.getByRole('button', { name: 'Download Schedule 01 (CSV)' }).click();
	const file = await download;
	expect(file.suggestedFilename()).toMatch(
		/^134012345_VAT_SCHEDULE01_\d{2}[1-4][0-4]_\d{8}_ORIGINAL_V1\.csv$/
	);
	const path = await file.path();
	const text = await (await import('node:fs/promises')).readFile(path, 'utf8');
	expect(text.split(/\r?\n/)[0]).toBe(
		"Serial No,Invoice Date,Tax Invoice No,Purchaser's TIN,Name of the Purchaser,Description,Value of supply,VAT Amount"
	);
});

test('a front desk user sees the daily lists but not the reports', async ({ page, browser }) => {
	const { email } = await signUp(page);
	await createProperty(page, 'RPD');
	const hotel = await bookableHotel(page, 2, 2);
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
	const propertyId = page.url().split('/p/')[1];
	const { email: clerk } = addStaff(email, 'front_desk', 'Ruwan Fernando');
	const { page: desk, context } = await signedIn(browser, clerk);

	await desk.goto(`/p/${propertyId}`);
	await expect(desk.getByRole('link', { name: 'Daily lists' })).toBeVisible();
	await expect(desk.getByRole('link', { name: 'Reports', exact: true })).toHaveCount(0);
	await expect(desk.getByRole('link', { name: 'Tax reports' })).toHaveCount(0);
	await desk.getByRole('link', { name: 'Daily lists' }).click();
	await expect(desk.getByRole('table', { name: 'Arrivals' })).toContainText(
		created.confirmation_no
	);
	await desk.getByRole('tab', { name: 'In-house' }).click();
	await expect(desk.getByText('No in-house on this day.')).toBeVisible();
	await context.close();
});

test('turning the weekly report email off persists after a reload', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'RPE');
	const propertyId = page.url().split('/p/')[1];

	await page.goto(`/p/${propertyId}/reports`);
	const weekly = page.getByRole('checkbox', { name: 'Weekly' });
	const monthly = page.getByRole('checkbox', { name: 'Monthly' });
	await expect(weekly).toBeChecked();
	const saved = page.waitForResponse(
		(response) => response.url().endsWith('/report-emails/weekly') && response.ok()
	);
	await weekly.uncheck();
	await saved;

	await page.reload();
	await expect(page.getByRole('checkbox', { name: 'Weekly' })).not.toBeChecked();
	await expect(monthly).toBeChecked();
});
