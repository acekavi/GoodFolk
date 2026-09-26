import { expect, test, type Page } from '@playwright/test';
import { addRoomType, createProperty, signUp } from './helpers';

/** Fills the rate plan editor and saves it. */
async function savePlan(
	page: Page,
	plan: {
		code?: string;
		name?: string;
		kind?: string;
		parent?: string;
		percent?: string;
		currency?: string;
		segment?: string;
	}
) {
	const editor = page.getByRole('form', { name: 'Rate plan' });
	if (plan.code) await editor.getByLabel('Code').fill(plan.code);
	if (plan.name) await editor.getByLabel('Name').fill(plan.name);
	if (plan.kind) await editor.getByLabel('Kind').selectOption(plan.kind);
	if (plan.parent) await editor.getByLabel('Derived from').selectOption({ label: plan.parent });
	if (plan.percent) await editor.getByLabel('Change (%)').fill(plan.percent);
	if (plan.currency) await editor.getByLabel('Currency').fill(plan.currency);
	if (plan.segment) await editor.getByLabel('Segment').selectOption(plan.segment);
	await editor.getByRole('button', { name: 'Save plan' }).click();
	await expect(editor).toBeHidden();
}

test('a revenue manager builds a tree of rate plans', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Room types' }).click();
	await addRoomType(page, 'DLX', 'Deluxe');

	await page.getByRole('link', { name: 'Rate plans' }).click();
	await page.getByRole('button', { name: 'New rate plan' }).click();
	await savePlan(page, { code: 'BAR', name: 'Best available', currency: 'USD', segment: 'FIT_F' });
	await page.getByRole('button', { name: 'New rate plan' }).click();
	await savePlan(page, {
		code: 'OTA',
		name: 'Online agents',
		kind: 'derived',
		parent: 'BAR',
		percent: '15',
		segment: 'OTA'
	});
	await page.getByRole('button', { name: 'New rate plan' }).click();
	await savePlan(page, { code: 'FITL', name: 'Residents', currency: 'LKR', segment: 'FIT_L' });

	const rows = page.getByRole('table', { name: 'Rate plans' }).getByRole('row');
	await expect(rows.nth(1)).toContainText('BAR');
	await expect(rows.nth(1)).toContainText('Non-residents');
	await expect(rows.nth(2)).toContainText('OTA');
	await expect(rows.nth(2)).toContainText('BAR + 15%');
	await expect(rows.nth(3)).toContainText('FITL');
	await expect(rows.nth(3)).toContainText('LKR');
	await expect(rows.nth(3)).toContainText('Residents only');

	await page.getByRole('button', { name: 'Edit OTA' }).click();
	await savePlan(page, { percent: '20' });
	await expect(rows.nth(2)).toContainText('BAR + 20%');

	// A derived plan takes its parent's currency, and a refused save says why.
	await page.getByRole('button', { name: 'New rate plan' }).click();
	const editor = page.getByRole('form', { name: 'Rate plan' });
	await editor.getByLabel('Kind').selectOption('derived');
	await editor.getByLabel('Derived from').selectOption({ label: 'FITL' });
	await expect(editor.getByLabel('Currency')).toHaveValue('LKR');
	await expect(editor.getByLabel('Currency')).toBeDisabled();
	await editor.getByLabel('Code').fill('BAR');
	await editor.getByLabel('Name').fill('Duplicate');
	await editor.getByLabel('Change (%)').fill('10');
	await editor.getByRole('button', { name: 'Save plan' }).click();
	await expect(editor.getByRole('alert')).toContainText('a rate plan with code BAR already exists');
});
