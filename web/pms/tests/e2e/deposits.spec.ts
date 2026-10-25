import { expect, test } from '@playwright/test';
import { bookableHotel, createProperty, signUp } from './helpers';

// The advisory deposit end to end, in a real browser against the real API: a plan with a 50% at-booking rule,
// the booking review's "Deposit due" line, the same amount in the reservation modal and the night audit, and a
// payment on the deposit folio clearing it.

test('a 50% at-booking deposit is shown on the review and the reservation, listed in the night audit, and cleared by a payment', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'DPS');
	const propertyId = page.url().split('/p/')[1];
	// BAR at USD 100 a night for two adults, so a 2 night stay is USD 200.
	const hotel = await bookableHotel(page, 2, 2);

	// The rule is set on the plan.
	await page.goto(`/p/${propertyId}/rate-plans`);
	await page.getByRole('button', { name: 'Edit BAR' }).click();
	const plan = page.getByRole('form', { name: 'Rate plan' });
	await plan.getByLabel('Asks for').selectOption('percent');
	await plan.getByLabel('Percentage (%)').fill('50');
	await expect(plan.getByLabel('Due')).toHaveValue('at_booking');
	await plan.getByRole('button', { name: 'Save plan' }).click();
	await expect(plan).toBeHidden();
	await expect(page.getByRole('row').filter({ hasText: 'BAR' })).toContainText(
		'50% of the stay, at booking'
	);

	// A booking through the new-reservation screen shows the deposit on its review step.
	await page.goto(`/p/${propertyId}/reservations/new`);
	await page.getByLabel('Nights').fill('2');
	await page.getByRole('radio', { name: 'Non-resident' }).check();
	await page.getByRole('button', { name: 'Search' }).click();
	await page.getByRole('radio', { name: /BAR · Room only/ }).check();
	await page.getByRole('button', { name: 'Continue' }).click();
	await page.getByLabel('Find a guest').fill('Silva');
	await page.getByLabel('Find a guest').press('Enter');
	await page.getByRole('button', { name: /Ada Silva/ }).click();
	const review = page.getByRole('region', { name: '4. Review' });
	const reviewed = review.getByText(/^Deposit due /);
	await expect(reviewed).toHaveText(`Deposit due USD 100.00 by ${hotel.businessDate}`);
	await page.getByRole('button', { name: 'Create reservation' }).click();

	// The reservation's modal asks for the same amount, nothing paid yet.
	const modal = page.getByRole('dialog', { name: /DPS-000001/ });
	await expect(modal).toBeVisible();
	await expect(modal.getByText(/^Due USD /)).toHaveText(
		`Due USD 100.00 by ${hotel.businessDate} · paid USD 0.00 · outstanding USD 100.00`
	);

	// The night audit lists it, as advisory.
	await page.goto(`/p/${propertyId}/night-audit`);
	await expect(page.getByRole('heading', { name: 'Deposits outstanding (1)' })).toBeVisible();
	const row = page.getByRole('region', { name: /Deposits outstanding/ }).getByRole('row', {
		name: /DPS-000001/
	});
	await expect(row).toContainText('Ada Silva');
	await expect(row).toContainText(`USD 100.00 by ${hotel.businessDate}`);

	// A payment on the deposit folio clears it everywhere.
	await page.goto(`/p/${propertyId}/reservations`);
	await page.getByRole('link', { name: 'DPS-000001' }).click();
	const reopened = page.getByRole('dialog', { name: /DPS-000001/ });
	await reopened.getByRole('tab', { name: 'Folios', exact: true }).click();
	await reopened.getByRole('tab', { name: 'Deposit', exact: true }).click();
	const deposit = reopened.getByRole('region', { name: 'Folio Deposit' });
	await deposit.getByRole('button', { name: 'Take payment' }).click();
	const payment = page.getByRole('dialog', { name: /Take a payment/ });
	await payment.getByLabel('Currency').selectOption('USD');
	await payment.getByLabel('Amount').fill('100');
	await payment.getByRole('button', { name: 'Take payment' }).click();
	await expect(payment).toBeHidden();
	await expect(deposit.getByRole('table', { name: 'Payments of Deposit' })).toContainText('Cash');
	await reopened.getByRole('tab', { name: 'Details', exact: true }).click();
	await expect(reopened.getByText(/^Due USD /)).toHaveText(
		`Due USD 100.00 by ${hotel.businessDate} · paid USD 100.00 · outstanding USD 0.00`
	);

	await page.goto(`/p/${propertyId}/night-audit`);
	await expect(page.getByRole('heading', { name: 'Deposits outstanding (0)' })).toBeVisible();
	await expect(page.getByText('No deposit past its due date is still owed.')).toBeVisible();
});
