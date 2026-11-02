import { expect, test, type Page } from '@playwright/test';
import {
	PMS,
	SITE_ORIGIN,
	addDays,
	addFrontDesk,
	mailTo,
	reservationCount,
	seedHotel,
	setSite,
	signIn,
	type Hotel
} from './helpers';

/** Two rooms for two adults each, from the day after the business date for two nights, picked and ready to book. */
async function pickTwoRooms(page: Page, hotel: Hotel) {
	await page.goto(`/${hotel.code}`);
	await page.getByLabel('No, I am visiting').check();
	await page.getByLabel('Check-in').fill(addDays(hotel.businessDate, 1));
	await page.getByLabel('Check-out').fill(addDays(hotel.businessDate, 3));
	// One room of two adults is the default; an added room starts with one.
	await page.getByRole('button', { name: 'Add a room' }).click();
	await page.getByRole('button', { name: 'One adult more in room 2' }).click();
	await page.getByRole('button', { name: 'Show rooms and prices' }).click();

	await expect(page.getByRole('heading', { name: 'Choose room 1 of 2' })).toBeVisible();
	await page.getByRole('button', { name: /^Choose Deluxe, Best available/ }).click();
	await expect(page.getByRole('heading', { name: 'Choose room 2 of 2' })).toBeVisible();
	await page.getByRole('button', { name: /^Choose Standard, Best available/ }).click();
	await page.getByRole('link', { name: 'Continue to your details' }).click();
	await expect(page.getByRole('heading', { name: 'Your details' })).toBeVisible();
}

/** Fills in the book page and waits for Turnstile's test widget to pass; the form is not submitted. */
async function fillDetails(page: Page, email: string) {
	await page.getByLabel('First name').fill('Anna');
	await page.getByLabel('Last name').fill('Schmidt');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Phone').fill('+49301234567');
	await page.getByLabel('Country of residence').selectOption('DE');
	await page.getByLabel('I agree to the hotel’s booking terms and privacy notice').check();
	await expect
		.poll(() => page.locator('input[name="cf-turnstile-response"]').inputValue(), {
			message: 'the security check (calls Cloudflare: needs the internet)',
			timeout: 30_000
		})
		.not.toBe('');
}

function guestEmail() {
	return `guest-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
}

test('a guest books two rooms, the front desk sees them, and the guest cancels from the email link', async ({
	page,
	browser
}) => {
	const owner = await browser.newContext({ baseURL: PMS });
	const hotel = await seedHotel(owner.request);
	const frontDesk = addFrontDesk(hotel.ownerEmail);
	const email = guestEmail();

	await pickTwoRooms(page, hotel);
	await expect(page.locator('.total')).toContainText('Total');
	await fillDetails(page, email);
	await page.getByRole('button', { name: 'Book now, pay at the hotel' }).click();

	await expect(page.getByRole('heading', { name: 'Your booking is confirmed' })).toBeVisible();
	const confirmation = page.url().split('/booked/')[1];
	expect(confirmation).toMatch(/^[A-Z0-9]+-\d+$/);
	await expect(page.getByText(`Confirmation number ${confirmation}`)).toBeVisible();
	await expect(page.getByText('Deluxe, Best available')).toBeVisible();
	await expect(page.getByText('Standard, Best available')).toBeVisible();

	// The confirmation email, with the link the cancel step below follows.
	const mail = await mailTo(email, 'Your booking at');
	expect(mail.subject).toBe(`Your booking at ${hotel.name}: ${confirmation}`);
	const manageLink = new RegExp(`${SITE_ORIGIN}/${hotel.code}/manage\\?token=[\\w-]+`).exec(
		mail.text
	)?.[0];
	expect(manageLink, mail.text).toBeDefined();

	// The front desk finds it on the reservations page, from the booking site.
	const staff = await browser.newContext({ baseURL: PMS });
	const desk = await staff.newPage();
	await signIn(desk, frontDesk);
	await desk.goto(`/p/${hotel.propertyId}/reservations`);
	const rows = desk.getByRole('row').filter({ hasText: confirmation });
	await expect(rows).toHaveCount(2);
	for (const row of await rows.all()) {
		await expect(row).toContainText('Booking site');
		await expect(row).toContainText('Schmidt');
		await expect(row).toContainText('Confirmed');
	}
	// The source filter: without the booking site, the rows go.
	await desk.getByRole('checkbox', { name: 'Booking site' }).uncheck();
	await expect(rows).toHaveCount(0);
	await desk.getByRole('checkbox', { name: 'Booking site' }).check();
	await expect(rows).toHaveCount(2);

	// The guest follows the email's link and cancels.
	const guest = await browser.newContext();
	const manage = await guest.newPage();
	await manage.goto(manageLink!);
	await expect(manage.getByRole('heading', { name: 'Your booking' })).toBeVisible();
	await manage.getByRole('button', { name: 'Cancel booking' }).click();
	await manage.getByRole('button', { name: 'Yes, cancel the booking' }).click();
	await expect(manage.getByText('Your booking is cancelled.')).toBeVisible();

	await desk.reload();
	await expect(rows).toHaveCount(2);
	for (const row of await rows.all()) await expect(row).toContainText('Cancelled');
});

test('a site that is off answers 404, and so does one that was turned off', async ({
	page,
	browser
}) => {
	const owner = await browser.newContext({ baseURL: PMS });
	const never = await seedHotel(owner.request, false);
	expect((await page.goto(`/${never.code}`))?.status()).toBe(404);

	const off = await seedHotel(owner.request);
	expect((await page.goto(`/${off.code}`))?.status()).toBe(200);
	await setSite(owner.request, off, false, 1);
	expect((await page.goto(`/${off.code}`))?.status()).toBe(404);
});

test('double-clicking Book makes one reservation', async ({ page, browser }) => {
	const owner = await browser.newContext({ baseURL: PMS });
	const hotel = await seedHotel(owner.request);

	await pickTwoRooms(page, hotel);
	await fillDetails(page, guestEmail());
	const posts: string[] = [];
	page.on('request', (request) => {
		if (request.method() === 'POST' && request.url().endsWith('/bookings')) {
			posts.push(request.url());
		}
	});
	await page.getByRole('button', { name: 'Book now, pay at the hotel' }).dblclick();
	await expect(page.getByRole('heading', { name: 'Your booking is confirmed' })).toBeVisible();

	// The button is disabled as the first click starts the booking, so the second click sends nothing: one request.
	expect(posts).toHaveLength(1);
	// One booking of two rooms is one reservation, and it stays one: a second booking still in flight would land
	// within these seconds.
	await expect.poll(() => reservationCount(hotel), { intervals: [500], timeout: 4000 }).toBe(1);
	await page.waitForTimeout(3000);
	expect(reservationCount(hotel)).toBe(1);
});

test('/favicon.ico on the booking domain answers 404', async ({ request }) => {
	expect((await request.get('/favicon.ico')).status()).toBe(404);
});

test('a path that is not a property code answers 404', async ({ request }) => {
	expect((await request.get('/x')).status()).toBe(404);
});
