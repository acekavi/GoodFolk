import { expect, test, type Locator } from '@playwright/test';
import { book, bookableHotel, createProperty, signUp } from './helpers';

const RESERVATIONS = 60;
const SOURCES = ['front_desk', 'phone', 'email'] as const;

/** The most rows the table may render: those in view, five of overscan each side, the header, and the
 * focused row wherever it is. */
async function rowBound(table: Locator): Promise<number> {
	const inView = await table.evaluate((scroller) => Math.ceil(scroller.clientHeight / 36) + 1);
	return inView + 2 * 5 + 2;
}

test('the reservations table pages over a cursor, sorts and filters on the server, and keeps its state in the URL', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	// Ten rooms over six nights: ten one-night reservations a night, GAL-000001 to GAL-000060.
	const hotel = await bookableHotel(page, 10, RESERVATIONS / 10);
	for (let night = 0; night < RESERVATIONS / 10; night++) {
		await Promise.all(
			Array.from({ length: 10 }, (_, index) =>
				book(page.request, hotel, [night], SOURCES[index % SOURCES.length])
			)
		);
	}

	await page.getByRole('link', { name: 'Reservations' }).click();
	const table = page.getByRole('table', { name: 'Reservations' });
	const rows = table.getByRole('row');
	const row = (index: number) => table.locator(`[role="row"][aria-rowindex="${index + 1}"]`);
	await expect(page.getByText(`${RESERVATIONS} reserved rooms`)).toBeVisible();
	await expect(table).toHaveAttribute('aria-rowcount', String(RESERVATIONS + 1));
	await expect(page.getByRole('link', { name: 'New reservation' })).toHaveAttribute(
		'href',
		/\/reservations\/new$/
	);

	// The first page is 50 rows, of which only those in view (plus overscan) are in the DOM.
	await expect(row(1)).toBeVisible();
	const bound = await rowBound(table);
	expect(await rows.count()).toBeLessThanOrEqual(bound);

	// Arrow keys move between the row links, and the focused row is scrolled into view.
	await row(1).getByRole('link').focus();
	for (let press = 0; press < 20; press++) await page.keyboard.press('ArrowDown');
	await expect(row(21).getByRole('link')).toBeFocused();
	await expect(row(21)).toBeInViewport();
	await page.keyboard.press('ArrowUp');
	await expect(row(20).getByRole('link')).toBeFocused();

	// Scrolling near the end loads the next page, which asks for no count (the first page's total stays);
	// the DOM still holds only the rows in view.
	const nextPage = page.waitForRequest((request) => {
		const body = request.postData() ?? '';
		return body.includes('ReservationList(') && body.includes('"withCount":false');
	});
	await expect
		.poll(async () => {
			await table.evaluate((scroller) => (scroller.scrollTop = scroller.scrollHeight));
			return row(RESERVATIONS).count();
		})
		.toBe(1);
	await nextPage;
	await expect(page.getByText(`${RESERVATIONS} reserved rooms`)).toBeVisible();
	expect(await rows.count()).toBeLessThanOrEqual(bound);

	// Opening a reservation keeps the table mounted, scrolled where it was, with its rows; so does Back.
	const scrolled = await table.evaluate((scroller) => scroller.scrollTop);
	await row(RESERVATIONS).getByRole('link').click();
	await expect(page).toHaveURL(/\/reservations\/[0-9a-f-]{36}$/);
	await expect(row(RESERVATIONS)).toBeVisible();
	expect(await table.evaluate((scroller) => scroller.scrollTop)).toBe(scrolled);
	await page.goBack();
	await expect(page).toHaveURL(/\/reservations$/);
	await expect(row(RESERVATIONS)).toBeVisible();
	expect(await table.evaluate((scroller) => scroller.scrollTop)).toBe(scrolled);

	// Sorting is done on the server: confirmation descending puts the last number first.
	const confirmation = table.getByRole('columnheader', { name: 'Confirmation #' });
	await confirmation.getByRole('button').click();
	await expect(confirmation).toHaveAttribute('aria-sort', 'ascending');
	await expect(row(1)).toContainText('GAL-000001');
	await confirmation.getByRole('button').click();
	await expect(confirmation).toHaveAttribute('aria-sort', 'descending');
	await expect(row(1)).toContainText('GAL-000060');

	// A confirmation prefix narrows the rows, lands in the URL and survives a reload.
	await page.getByLabel('Search').fill('gal-00005');
	await expect(page.getByText('10 reserved rooms')).toBeVisible();
	await expect(page).toHaveURL(/[?&]text=gal-00005(&|$)/);
	await expect(page).toHaveURL(/[?&]sort=CONFIRMATION&dir=DESC(&|$)/);
	await expect(rows).toHaveCount(11);
	await expect(row(1)).toContainText('GAL-000059');
	await page.reload();
	await expect(page.getByLabel('Search')).toHaveValue('gal-00005');
	await expect(page.getByText('10 reserved rooms')).toBeVisible();
	await expect(confirmation).toHaveAttribute('aria-sort', 'descending');
	await expect(row(1)).toContainText('GAL-000059');

	// Statuses: with every room confirmed, leaving out Confirmed leaves nothing.
	await page.getByRole('checkbox', { name: 'Confirmed' }).uncheck();
	await expect(page).toHaveURL(/[?&]statuses=TENTATIVE%2CCHECKED_IN/);
	await expect(page.getByText('No reservations match these filters.')).toBeVisible();
	await page.getByRole('checkbox', { name: 'Confirmed' }).check();
	await expect(page).not.toHaveURL(/statuses=/);

	// Pointing at a row fetches its reservation ahead of the click.
	const detail = page.waitForRequest(
		(request) =>
			request.url().endsWith('/graphql') && (request.postData() ?? '').includes('Reservation(')
	);
	await row(2).hover();
	await detail;
});
