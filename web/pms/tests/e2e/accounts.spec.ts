import { expect, test } from '@playwright/test';
import { createProperty, signUp } from './helpers';

test('a front desk user manages company and travel agent accounts', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Accounts' }).click();
	await expect(page.getByRole('heading', { name: 'Accounts' })).toBeVisible();

	// Create a company account.
	const form = page.getByRole('form', { name: 'New account' });
	await form.getByLabel('Name', { exact: true }).fill('Acme Corp');
	await form.getByLabel('Kind').selectOption('company');
	await form.getByLabel('Email').fill('billing@acme.test');
	await form.getByLabel('Currency').fill('USD');
	await form.getByRole('button', { name: 'Add account' }).click();

	const table = page.getByRole('table', { name: 'Accounts' });
	const row = table.getByRole('row', { name: 'Acme Corp' });
	await expect(row).toBeVisible();
	await expect(row).toContainText('Company');
	await expect(row).toContainText('billing@acme.test');
	await expect(row).toContainText('No limit');
	await expect(row).toContainText('USD');
	await expect(row).toContainText('Active');

	// Edit its credit limit.
	await row.getByRole('button', { name: 'Edit Acme Corp' }).click();
	await page.getByLabel('Credit limit for Acme Corp').fill('5000');
	await page.getByRole('button', { name: 'Save Acme Corp' }).click();
	await expect(row).toContainText('5,000.00');

	// Deactivate: the row disappears until "Show inactive" is switched on.
	await row.getByRole('button', { name: 'Deactivate Acme Corp' }).click();
	await expect(table.getByRole('row', { name: 'Acme Corp' })).toBeHidden();
	await page.getByLabel('Show inactive').check();
	await expect(row).toBeVisible();
	await expect(row).toContainText('Inactive');
	await row.getByRole('button', { name: 'Activate Acme Corp' }).click();
	await expect(row).toContainText('Active');

	// Account names are not unique: a second account with the same name is fine.
	await form.getByLabel('Name', { exact: true }).fill('Acme Corp');
	await form.getByLabel('Kind').selectOption('travel_agent');
	await form.getByLabel('Currency').fill('USD');
	await form.getByRole('button', { name: 'Add account' }).click();
	await expect(table.getByRole('row', { name: 'Acme Corp' })).toHaveCount(2);

	// A validation error (credit limit out of range) shows the server's message.
	await form.getByLabel('Name', { exact: true }).fill('Overlimit Travel');
	await form.getByLabel('Kind').selectOption('travel_agent');
	await form.getByLabel('Currency').fill('USD');
	await form.getByLabel('Credit limit').fill('200000000000');
	await form.getByRole('button', { name: 'Add account' }).click();
	await expect(page.getByRole('alert')).toContainText('credit_limit');
	await expect(table.getByRole('row', { name: 'Overlimit Travel' })).toHaveCount(0);
});
