import { expect, test, type Browser, type Page, type Request } from '@playwright/test';
import {
	addDays,
	addStaff,
	bookableHotel,
	createProperty,
	post,
	runAsOwner,
	runsQuery,
	signIn,
	signUp,
	type Hotel
} from './helpers';

// Folios end to end, in a real browser against the real API, media-svc (which renders the invoice PDFs) and
// RustFS: charges, payments, invoices, POS bills, deposits, routing, the settings pages and tonight's charges.
// Needs RustFS on :9000 (global-setup.ts checks it); Playwright starts media-svc. Playwright also starts
// core-api with FX_IMPORT=off, and each property is given its own exchange rates (see `seedRates`).

/** The PDF is rendered by media-svc, so it takes a moment to be ready. */
const READY = { timeout: 30_000 };

/** A signed-up owner's property, bookable over the REST API, with the page left on it. */
async function hotel(page: Page, code: string, rooms = 2, nights = 1) {
	const { email } = await signUp(page);
	await createProperty(page, code);
	const propertyId = page.url().split('/p/')[1];
	return { email, propertyId, ...(await bookableHotel(page, rooms, nights)) };
}

/** Books one DLX room for the business date's night, to `account` if given, and checks the stay in. */
async function bookStay(
	page: Page,
	hotel: Hotel,
	options: { account?: string; checkIn?: boolean } = {}
) {
	const created = await post(page.request, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		account_id: options.account,
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
	if (options.checkIn ?? true) {
		const checkedIn = await page.request.post(
			`${hotel.path}/reservation-rooms/${room.id}/check-in`,
			{
				headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${room.version}"` }
			}
		);
		expect(checkedIn.status(), await checkedIn.text()).toBe(200);
	}
	return {
		id: created.id as string,
		confirmation: created.confirmation_no as string,
		stay: room.id
	};
}

/** Opens the reservation's modal on its Folios tab, on the folio tab `folio` (the stay's room), and returns the modal. */
async function openFolios(
	page: Page,
	propertyId: string,
	reservation: { id: string; confirmation: string },
	folio = 'Room 101'
) {
	await page.goto(`/p/${propertyId}/reservations/${reservation.id}`);
	const modal = page.getByRole('dialog', { name: new RegExp(reservation.confirmation) });
	await modal.getByRole('tab', { name: 'Folios', exact: true }).click();
	await modal.getByRole('tab', { name: folio, exact: true }).click();
	await expect(modal.getByRole('region', { name: `Folio ${folio}` })).toBeVisible();
	return modal;
}

/** The tenant of the signed-in user. */
async function tenantOf(page: Page): Promise<string> {
	const me = await page.request.get('/api/v1/me');
	expect(me.status()).toBe(200);
	return (await me.json()).current_tenant;
}

/** How many audit entries of `action` the tenant has, read as the schema owner. */
function audited(tenant: string, action: string): number {
	const printed = runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		const [{ count }] = await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			return tx\`select count(*)::int as count from audit_log
				where tenant_id = \${process.env.TENANT} and action = \${process.env.ACTION}\`;
		});
		await sql.close();
		// A string, which is never coloured, even under FORCE_COLOR.
		console.log(String(count));
	`,
		{ TENANT: tenant, ACTION: action }
	);
	return Number(printed.trim());
}

/** A page of `email`, signed in, in a context of its own. */
async function signedIn(browser: Browser, email: string) {
	const context = await browser.newContext();
	const page = await context.newPage();
	await signIn(page, email);
	return { page, context };
}

test('a charge is posted with its taxes and a cash payment settles the folio', async ({ page }) => {
	const owner = await hotel(page, 'CHG');
	const reservation = await bookStay(page, owner);
	const modal = await openFolios(page, owner.propertyId, reservation);
	const folio = modal.getByRole('region', { name: 'Folio Room 101' });
	await expect(folio.getByText('No charges.', { exact: true })).toBeVisible();

	await folio.getByRole('button', { name: 'Post charge' }).click();
	const charge = page.getByRole('dialog', { name: /Post a charge/ });
	await charge.getByLabel('Description').fill('Minibar');
	await charge.getByLabel('Unit price before taxes').fill('1,000');
	await charge.getByRole('button', { name: 'Post charge' }).click();
	await expect(charge).toBeHidden();

	const lines = folio.getByRole('table', { name: 'Lines of Room 101' });
	const line = lines.getByRole('row', { name: /Minibar/ });
	await expect(line).toContainText('Miscellaneous');
	await expect(folio.getByRole('list', { name: 'Balances' })).toContainText('due');
	await line.getByRole('button', { name: 'Taxes' }).click();
	const taxes = lines.locator('dl.taxes');
	await expect(taxes).toContainText('Net');
	await expect(taxes).toContainText('LKR 1,000.00');
	await expect(taxes).toContainText('Tourism development levy 1%');
	await expect(taxes).toContainText('LKR 10.00');

	await folio.getByRole('button', { name: 'Take payment' }).click();
	const payment = page.getByRole('dialog', { name: /Take a payment/ });
	await expect(payment.getByLabel('Method')).toHaveValue('cash');
	await payment.getByRole('button', { name: 'Take payment' }).click();
	await expect(payment).toBeHidden();
	await expect(folio.getByRole('table', { name: 'Payments of Room 101' })).toContainText('Cash');
	await expect(folio.getByRole('list', { name: 'Balances' })).toContainText('settled');
});

test('an invoice is issued, its PDF is ready live and opens, and a credit note reopens the folio for a new invoice', async ({
	page
}) => {
	const owner = await hotel(page, 'INV');
	const reservation = await bookStay(page, owner);
	const year = owner.businessDate.slice(0, 4);
	const modal = await openFolios(page, owner.propertyId, reservation);
	const folio = modal.getByRole('region', { name: 'Folio Room 101' });

	await folio.getByRole('button', { name: 'Post charge' }).click();
	const charge = page.getByRole('dialog', { name: /Post a charge/ });
	await charge.getByLabel('Description').fill('Airport transfer');
	await charge.getByLabel('Unit price before taxes').fill('2,000');
	await charge.getByRole('button', { name: 'Post charge' }).click();
	await expect(charge).toBeHidden();
	const issue = folio.getByRole('button', { name: 'Issue invoice' });
	await expect(issue).toBeDisabled();
	await expect(folio).toContainText('Settle the balance first');

	await folio.getByRole('button', { name: 'Take payment' }).click();
	const payment = page.getByRole('dialog', { name: /Take a payment/ });
	await payment.getByRole('button', { name: 'Take payment' }).click();
	await expect(payment).toBeHidden();
	await expect(issue).toBeEnabled();

	// A mark on the window shows the page is never reloaded on the way to a ready PDF.
	await page.evaluate(() => Object.assign(window, { unreloaded: true }));
	await issue.click();
	const invoices = folio.getByRole('list', { name: 'Invoices of Room 101' });
	const serial = `INV-${year}-000001`;
	await expect(invoices.getByText(serial, { exact: true })).toBeVisible();
	await expect(folio.getByRole('button', { name: `View invoice ${serial}` })).toBeVisible(READY);
	await expect(folio.getByText('Preparing PDF…')).toHaveCount(0);
	expect(
		await page.evaluate(() => (window as unknown as { unreloaded?: boolean }).unreloaded)
	).toBe(true);
	await expect(folio).toContainText('Invoiced: it takes no charges');

	// The PDF opens in a new tab, from a fresh link, as a PDF.
	const pdf = page.context().waitForEvent('response', {
		predicate: (response) => response.url().startsWith('http://localhost:9000/')
	});
	const popup = page.waitForEvent('popup');
	await folio.getByRole('button', { name: `View invoice ${serial}` }).click();
	await popup;
	const response = await pdf;
	expect(response.status()).toBe(200);
	expect(response.headers()['content-type']).toBe('application/pdf');
	expect(audited(await tenantOf(page), 'invoice.viewed')).toBe(1);

	// A credit note reopens the folio, and the next invoice has the next serial.
	await folio.getByRole('button', { name: `Credit note for ${serial}` }).click();
	await folio.getByLabel(`Why credit ${serial}?`).fill('Wrong purchaser');
	await folio.getByRole('button', { name: 'Issue credit note' }).click();
	await expect(invoices.getByText(`CN-${serial}`, { exact: true })).toBeVisible();
	await expect(folio).not.toContainText('Invoiced: it takes no charges');
	await expect(issue).toBeEnabled();
	await issue.click();
	await expect(invoices.getByText(`INV-${year}-000002`, { exact: true })).toBeVisible();
});

test('a POS bill posted to a room shows on the guest folio with its service charge, and a replay adds nothing', async ({
	page
}) => {
	const owner = await hotel(page, 'POS');
	const reservation = await bookStay(page, owner);
	await post(page.request, `${owner.path}/outlets`, { code: 'BAR', name: 'Bar' });
	const bill = {
		outlet: 'BAR',
		bill_no: 'B-1',
		currency: 'LKR',
		items: [{ description: 'Club sandwich', quantity: 1, unit_net: 100_000 }],
		target: { reservation_room: reservation.stay }
	};
	const key = crypto.randomUUID();
	const send = (idempotencyKey: string) =>
		page.request.post(`${owner.path}/pos-bills`, {
			headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': idempotencyKey },
			data: bill
		});
	const first = await send(key);
	expect(first.status(), await first.text()).toBe(201);
	const posted = await first.json();
	const same = await send(key);
	expect(same.status(), await same.text()).toBeLessThan(300);
	expect((await same.json()).id).toBe(posted.id);
	// The same bill under a new key is the bill already posted.
	const again = await send(crypto.randomUUID());
	expect(again.status(), await again.text()).toBe(200);
	expect((await again.json()).id).toBe(posted.id);

	const modal = await openFolios(page, owner.propertyId, reservation);
	const lines = modal.getByRole('table', { name: 'Lines of Room 101' });
	const line = lines.getByRole('row', { name: /Club sandwich/ });
	await expect(line).toHaveCount(1);
	await expect(line).toContainText('Bar (POS)');
	await line.getByRole('button', { name: 'Taxes' }).click();
	await expect(lines.locator('dl.taxes')).toContainText('Service charge 10%');
});

test('a deposit moves from the deposit folio to the stay at check-in', async ({ page }) => {
	const owner = await hotel(page, 'DEP');
	const reservation = await bookStay(page, owner, { checkIn: false });
	let modal = await openFolios(page, owner.propertyId, reservation, 'Deposit');
	const deposit = modal.getByRole('region', { name: 'Folio Deposit' });
	await deposit.getByRole('button', { name: 'Take payment' }).click();
	const payment = page.getByRole('dialog', { name: /Take a payment/ });
	await payment.getByLabel('Amount').fill('5,000');
	await payment.getByRole('button', { name: 'Take payment' }).click();
	await expect(payment).toBeHidden();
	await expect(deposit.getByRole('list', { name: 'Balances' })).toContainText('-5,000.00');
	await expect(deposit.getByRole('list', { name: 'Balances' })).toContainText('in credit');

	const room = reservation.stay;
	const checkedIn = await page.request.post(`${owner.path}/reservation-rooms/${room}/check-in`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' }
	});
	expect(checkedIn.status(), await checkedIn.text()).toBe(200);

	modal = await openFolios(page, owner.propertyId, reservation, 'Deposit');
	const moved = modal.getByRole('region', { name: 'Folio Deposit' });
	await expect(moved.getByRole('list', { name: 'Balances' })).toContainText('settled');
	await expect(moved.getByRole('table', { name: 'Payments of Deposit' })).toContainText('Moved');
	await modal.getByRole('tab', { name: 'Room 101', exact: true }).click();
	const guest = modal.getByRole('region', { name: 'Folio Room 101' });
	await expect(guest.getByRole('list', { name: 'Balances' })).toContainText('-5,000.00');
	await expect(guest.getByRole('list', { name: 'Balances' })).toContainText('in credit');
	await expect(guest.getByRole('table', { name: 'Payments of Room 101' })).toContainText('Moved');
});

test('a housekeeper sees no Folios tab and never asks for folios', async ({ page, browser }) => {
	const owner = await hotel(page, 'FHK');
	const reservation = await bookStay(page, owner);
	const { email: housekeeper } = addStaff(owner.email, 'housekeeping', 'Kumari Silva');
	const { page: hk, context } = await signedIn(browser, housekeeper);
	const asked: Request[] = [];
	hk.on('request', (request) => {
		if (request.url().endsWith('/graphql') && runsQuery(request, 'Folios')) asked.push(request);
	});

	// Who may see what is known once the profile has loaded.
	const profile = hk.waitForResponse((response) => response.url().endsWith('/api/v1/me'));
	await hk.goto(`/p/${owner.propertyId}/reservations/${reservation.id}`);
	expect((await profile).ok()).toBe(true);
	const modal = hk.getByRole('dialog', { name: new RegExp(reservation.confirmation) });
	await expect(modal).toContainText('Ada Silva');
	await expect(modal.getByRole('table', { name: 'History' })).toBeVisible();
	await expect(modal.getByRole('tab', { name: 'Folios', exact: true })).toHaveCount(0);
	expect(asked).toHaveLength(0);
	await context.close();
});

test('routing sends the room charge to the account folio, which the reservation and the accounts page show', async ({
	page
}) => {
	const owner = await hotel(page, 'RTE');
	const account = await post(page.request, `${owner.path}/accounts`, {
		kind: 'company',
		name: 'Acme Corp',
		currency: 'USD'
	});
	const reservation = await bookStay(page, owner, { account: account.id });
	let modal = await openFolios(page, owner.propertyId, reservation);
	await modal.getByRole('button', { name: 'Routing' }).click();
	const routing = page.getByRole('dialog', { name: 'Routing' });
	await routing.getByLabel('Room', { exact: true }).check();
	await routing.getByRole('button', { name: 'Save routing' }).click();
	await expect(routing).toBeHidden();
	await modal.getByRole('button', { name: 'Close' }).click();
	await expect(modal).toBeHidden();

	page.on('dialog', (dialog) => void dialog.accept());
	await page.getByRole('button', { name: "Post tonight's charges" }).click();
	await expect(page.getByRole('status', { name: "Tonight's charges" })).toContainText(
		'1 line posted for 1 stay'
	);

	// The stay's own folio has no charge; the account's does, and the reservation shows it as shared.
	modal = await openFolios(page, owner.propertyId, reservation);
	await expect(modal.getByRole('region', { name: 'Folio Room 101' })).toContainText('No charges.');
	await modal.getByRole('tab', { name: 'Account · Acme Corp', exact: true }).click();
	await expect(modal.getByRole('region', { name: 'Folio Account · Acme Corp' })).toContainText(
		"This company's ledger is shared"
	);

	await page.goto(`/p/${owner.propertyId}/accounts`);
	await page.getByRole('button', { name: 'Folio of Acme Corp' }).click();
	const ledger = page.getByRole('region', { name: 'Folio Account · Acme Corp' });
	await expect(ledger.getByRole('table', { name: 'Lines of Account · Acme Corp' })).toContainText(
		'Room'
	);
	await expect(ledger.getByRole('list', { name: 'Balances' })).toContainText('due');
});

test('the Sri Lanka preset starts tomorrow, outlet codes are fixed, a rate is overridden, and only managers see the exchange-rate alert', async ({
	page,
	browser
}) => {
	const owner = await hotel(page, 'SET');

	// Taxes: the preset confirms, then shows rules starting tomorrow.
	const tomorrow = addDays(owner.businessDate, 1);
	await page.goto(`/p/${owner.propertyId}/settings/taxes`);
	const confirms: string[] = [];
	page.once('dialog', (dialog) => {
		confirms.push(dialog.message());
		void dialog.accept();
	});
	await page.getByRole('button', { name: 'Apply Sri Lanka preset' }).click();
	await expect(page.getByRole('status').filter({ hasText: 'takes effect on' })).toContainText(
		`The Sri Lanka preset takes effect on ${tomorrow}`
	);
	expect(confirms).toHaveLength(1);
	expect(confirms[0]).toContain(`tomorrow (${tomorrow})`);
	await expect(page.getByRole('heading', { name: `Upcoming · from ${tomorrow}` })).toBeVisible();
	await expect(page.getByRole('table', { name: `Rules from ${tomorrow}` })).toContainText('VAT');

	// Outlets: a new outlet's name can be edited, its code cannot.
	await page.goto(`/p/${owner.propertyId}/settings/outlets`);
	const form = page.getByRole('form', { name: 'New outlet' });
	await form.getByLabel('Code').fill('POOL');
	await form.getByLabel('Name').fill('Pool bar');
	await form.getByRole('button', { name: 'Add outlet' }).click();
	const row = page.getByRole('table', { name: 'Outlets' }).getByRole('row', { name: /POOL/ });
	await expect(row).toContainText('Pool bar');
	await row.getByRole('button', { name: 'Edit POOL' }).click();
	await expect(row.getByRole('textbox')).toHaveCount(1);
	await row.getByLabel('Name for POOL').fill('Pool side');
	await row.getByRole('button', { name: 'Save POOL' }).click();
	await expect(row).toContainText('Pool side');
	await expect(row.getByRole('textbox')).toHaveCount(0);
	await expect(page.getByText("can't be changed later")).toBeVisible();

	// Exchange rates: a manual USD rate for the business date.
	await page.goto(`/p/${owner.propertyId}/settings/exchange-rates`);
	const rate = page.getByRole('form', { name: 'Set a rate' });
	await rate.getByLabel('Currency').selectOption('USD');
	await rate.getByLabel('Rate').fill('305.5');
	await rate.getByRole('button', { name: 'Set rate' }).click();
	await expect(page.getByRole('status').filter({ hasText: 'is now 305.5' })).toBeVisible();
	const usd = page.getByRole('table', { name: 'USD rates' });
	await expect(usd.getByRole('row', { name: new RegExp(owner.businessDate) })).toContainText(
		'manual'
	);
	await expect(usd.getByRole('row', { name: new RegExp(owner.businessDate) })).toContainText(
		'305.5'
	);

	// FX banner: an alert row shows the owner a banner, and a front desk user none.
	const tenant = await tenantOf(page);
	runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			await tx\`insert into fx_alert (tenant_id, property_id, message)
				values (\${process.env.TENANT}, \${process.env.PROPERTY}, 'The Central Bank page changed')\`;
		});
		await sql.close();
	`,
		{ TENANT: tenant, PROPERTY: owner.propertyId }
	);
	await page.goto(`/p/${owner.propertyId}/rooms`);
	await expect(
		page.getByRole('alert').filter({ hasText: 'Exchange rates: The Central Bank page changed' })
	).toBeVisible();

	const { email: frontDesk } = addStaff(owner.email, 'front_desk', 'Ruwan Fernando');
	const { page: desk, context } = await signedIn(browser, frontDesk);
	const asked: Request[] = [];
	desk.on('request', (request) => {
		if (request.url().endsWith('/graphql') && runsQuery(request, 'FxAlert')) asked.push(request);
	});
	const profile = desk.waitForResponse((response) => response.url().endsWith('/api/v1/me'));
	await desk.goto(`/p/${owner.propertyId}/rooms`);
	expect((await profile).ok()).toBe(true);
	await expect(desk.getByRole('link', { name: 'Reservations' })).toBeVisible();
	await expect(desk.getByText('The Central Bank page changed')).toHaveCount(0);
	expect(asked).toHaveLength(0);
	await context.close();
});

test("tonight's charges are posted for every checked-in stay once", async ({ page }) => {
	const owner = await hotel(page, 'TON', 2, 1);
	await bookStay(page, owner);
	await bookStay(page, owner);
	page.on('dialog', (dialog) => void dialog.accept());
	await page.goto(`/p/${owner.propertyId}/reservations`);
	const button = page.getByRole('button', { name: "Post tonight's charges" });
	const result = page.getByRole('status', { name: "Tonight's charges" });

	await button.click();
	await expect(result).toContainText('2 lines posted for 2 stays; 0 already posted.');
	await button.click();
	await expect(result).toContainText('0 lines posted for 2 stays; 2 already posted.');
});
