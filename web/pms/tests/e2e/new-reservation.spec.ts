import { expect, test, type Page } from '@playwright/test';
import { addDays, book, bookableHotel, createProperty, signUp } from './helpers';

const ID_NUMBER = 'P98765432';

/** Presses Tab and expects the focus on the field labelled `label`. */
async function tabTo(page: Page, label: string) {
	await page.keyboard.press('Tab');
	await expect(page.getByLabel(label)).toBeFocused();
}

/** Opens the new-reservation screen from the reservations table, from the keyboard. */
async function openNewReservation(page: Page) {
	await page.getByRole('link', { name: 'Reservations' }).click();
	await page.getByRole('link', { name: 'New reservation' }).focus();
	await page.keyboard.press('Enter');
	await expect(page.getByRole('heading', { name: 'New reservation' })).toBeVisible();
}

test('a reservation is booked from the keyboard: stay, offer, a new guest with an ID, review, create', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'NEW');
	// One DLX room, BAR at USD 100 a night for two adults.
	const hotel = await bookableHotel(page, 1, 4);
	await openNewReservation(page);

	// The stay: check-in defaults to the business date; one more night moves the check-out.
	const checkIn = page.getByLabel('Check-in');
	await expect(checkIn).toHaveValue(hotel.businessDate);
	await page.getByLabel('Nights').focus();
	await page.keyboard.press('ArrowUp');
	await expect(page.getByLabel('Nights')).toHaveValue('2');
	await expect(page.getByLabel('Check-out')).toHaveValue(addDays(hotel.businessDate, 2));
	await tabTo(page, 'Adults');
	await expect(page.getByLabel('Adults')).toHaveValue('2');
	await tabTo(page, 'Children');
	// Residency has no default: Enter doesn't search, and the browser points at the missing choice.
	await page.keyboard.press('Enter');
	await expect(page.getByRole('heading', { name: '2. Offer' })).toBeHidden();
	await expect(page.getByRole('radio', { name: 'Resident', exact: true })).toBeFocused();
	await page.keyboard.press('ArrowDown');
	await expect(page.getByRole('radio', { name: 'Non-resident' })).toBeChecked();
	await page.keyboard.press('Enter');

	// The offers, by room type: focus moves to them, Tab reaches the first offer, Enter takes it.
	const offers = page.getByRole('group', { name: 'DLX · Deluxe · 1 free' });
	await expect(offers).toBeVisible();
	await expect(page.getByRole('heading', { name: '2. Offer' })).toBeFocused();
	const offer = offers.getByRole('radio', { name: /BAR · Room only/ });
	await expect(offer).toHaveAccessibleName('BAR · Room only USD 200.00');
	await page.keyboard.press('Tab');
	await expect(offer).toBeFocused();
	await page.keyboard.press('Space');
	// Space only moves the radio's checked state; it must not also take the offer and skip ahead.
	await expect(offer).toBeChecked();
	await expect(page.getByLabel('Find a guest')).toBeHidden();
	await page.keyboard.press('Enter');

	// A new guest, with an ID number; residency comes prefilled from the stay.
	await expect(page.getByLabel('Find a guest')).toBeFocused();
	await page.keyboard.press('Tab');
	await expect(page.getByRole('button', { name: 'New guest…' })).toBeFocused();
	await page.keyboard.press('Enter');
	const guest = page.getByRole('form', { name: 'New guest' });
	await expect(guest.getByLabel('First name')).toBeFocused();
	await page.keyboard.type('Grace');
	await tabTo(page, 'Last name');
	await page.keyboard.type('Hopper');
	await tabTo(page, 'Email');
	await page.keyboard.type('grace@example.com');
	await tabTo(page, 'Phone');
	await tabTo(page, 'Country');
	await page.keyboard.type('us');
	await tabTo(page, 'Guest residency');
	await expect(guest.getByLabel('Guest residency')).toHaveValue('non_resident');
	await tabTo(page, 'ID document');
	await page.keyboard.press('ArrowDown');
	await expect(guest.getByLabel('ID document')).toHaveValue('passport');
	await tabTo(page, 'ID number');
	await page.keyboard.type(ID_NUMBER);
	await page.keyboard.press('Enter');

	// The review: the stay, the offer, the guest with the ID masked, the total; then create.
	const review = page.getByRole('region', { name: '4. Review' });
	await expect(page.getByRole('heading', { name: '4. Review' })).toBeFocused();
	await expect(review).toContainText('2 nights');
	await expect(review).toContainText('DLX · Deluxe · BAR · Room only');
	await expect(review).toContainText('Grace Hopper · Non-resident · Passport •••• 5432');
	await expect(review).toContainText('USD 200.00');
	await expect(review).not.toContainText(ID_NUMBER);
	await tabTo(page, 'Rooms');
	await expect(page.getByLabel('Rooms')).toHaveAttribute('max', '1');
	await tabTo(page, 'Source');
	await expect(page.getByLabel('Source')).toHaveValue('FRONT_DESK');
	await tabTo(page, 'Notes');
	await page.keyboard.type('Late arrival');
	await page.keyboard.press('Tab');
	await expect(page.getByRole('button', { name: 'Create reservation' })).toBeFocused();
	await page.keyboard.press('Enter');

	// The new reservation's modal, over the table; closing it shows the table with its row.
	const dialog = page.getByRole('dialog', { name: 'NEW-000001 · Confirmed' });
	await expect(dialog).toBeVisible();
	await expect(page).toHaveURL(/\/reservations\/[0-9a-f-]{36}$/);
	await expect(dialog).toContainText('Grace Hopper');
	await expect(dialog).toContainText('Passport •••• 5432');
	await expect(dialog).toContainText('Late arrival');
	await expect(dialog).not.toContainText(ID_NUMBER);
	await page.keyboard.press('Escape');
	await expect(dialog).toBeHidden();
	await expect(page).toHaveURL(/\/reservations$/);
	const table = page.getByRole('table', { name: 'Reservations' });
	await expect(table.getByRole('link', { name: 'NEW-000001' })).toBeVisible();
	await expect(table).toContainText('Grace Hopper');

	// A second booking for the same nights finds DLX sold out: its offers can't be taken.
	await page.getByRole('link', { name: 'New reservation' }).click();
	await page.getByLabel('Nights').fill('2');
	await page.getByRole('radio', { name: 'Non-resident' }).check();
	await page.getByRole('button', { name: 'Search' }).click();
	const soldOut = page.getByRole('group', { name: 'DLX · Deluxe · Sold out' });
	await expect(soldOut).toBeVisible();
	await expect(soldOut.getByRole('radio', { name: /BAR · Room only/ })).toBeDisabled();
});

test('a guest of another residency than the stay is caught, and a room sold meanwhile sends the booking back to the offers', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'MIX');
	// One DLX room, and the non-resident guest Ada Silva.
	const hotel = await bookableHotel(page, 1, 2);
	await openNewReservation(page);

	// Searched for a resident; the offer is taken with a click.
	await page.getByRole('radio', { name: 'Resident', exact: true }).check();
	await page.getByRole('button', { name: 'Search' }).click();
	// Each offer's nights are a click away.
	await page.getByText('Nightly prices').click();
	const nightly = page.getByRole('table', { name: 'Nightly prices' });
	await expect(nightly).toContainText(hotel.businessDate);
	await expect(nightly).toContainText('100.00');
	await page.getByRole('radio', { name: /BAR · Room only/ }).click();

	// Ada Silva is a non-resident: she isn't taken, and the offers must be searched again for her.
	await page.getByLabel('Find a guest').fill('Silva');
	await page.getByRole('button', { name: /Ada Silva/ }).click();
	const mismatch = page.getByRole('alert');
	await expect(mismatch).toContainText(
		'Ada Silva is a non-resident, but the offers were searched for a resident.'
	);
	await expect(page.getByRole('region', { name: '4. Review' })).toBeHidden();
	await mismatch.getByRole('button', { name: 'Search again as a non-resident' }).click();
	await expect(page.getByRole('radio', { name: 'Non-resident' })).toBeChecked();
	await expect(page.getByRole('heading', { name: '2. Offer' })).toBeFocused();

	// The offer for her residency goes straight to the review, with her as the guest.
	await page.getByRole('radio', { name: /BAR · Room only/ }).click();
	const review = page.getByRole('region', { name: '4. Review' });
	await expect(review).toContainText('Ada Silva · Non-resident · None on file');

	// Meanwhile the last room is booked: creating is refused and the offers come back, sold out.
	await book(page.request, hotel, [0]);
	await review.getByRole('button', { name: 'Create reservation' }).click();
	await expect(page.getByRole('alert')).toHaveText(`no DLX rooms left on ${hotel.businessDate}`);
	await expect(review).toBeHidden();
	await expect(page.getByRole('group', { name: 'DLX · Deluxe · Sold out' })).toBeVisible();
});
