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

/** `YYYY-MM-DD` plus `days`. */
function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

/** The first day of the month after `date`'s, and the first Saturday and Monday in it. */
function nextMonth(date: string) {
	const first = new Date(`${date.slice(0, 7)}-01T00:00:00Z`);
	first.setUTCMonth(first.getUTCMonth() + 1);
	const start = first.toISOString().slice(0, 10);
	const offset = (weekday: number) => (weekday - first.getUTCDay() + 7) % 7;
	const last = new Date(first);
	last.setUTCMonth(last.getUTCMonth() + 1);
	last.setUTCDate(0);
	return {
		start,
		end: last.toISOString().slice(0, 10),
		saturday: addDays(start, offset(6)),
		monday: addDays(start, offset(1))
	};
}

test('prices are edited in the grid, changed in bulk and quoted', async ({ page }) => {
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

	await page.getByRole('link', { name: 'Rates', exact: true }).click();
	const today = (await page.getByTestId('business-date').textContent())!.trim();
	const grid = page.getByRole('grid', { name: 'Prices' });
	const cell = (row: string, date: string, text: string) =>
		grid.getByRole('gridcell', { name: `${row} ${date}: ${text}` });

	// Type a price into a cell; it is saved when the cell is left.
	await cell('DLX · 2 adults', today, 'no price').click();
	await page.getByLabel(`Price for DLX · 2 adults on ${today}`).fill('150');
	await page.keyboard.press('Enter');
	await expect(cell('DLX · 2 adults', today, '150.00')).toBeVisible();

	// The derived plan follows at once: 150.00 + 15% = 172.50, rounded to 173.00.
	await page.getByLabel('Rate plan').selectOption({ label: 'OTA' });
	await expect(page.getByText('Derived from BAR: BAR + 15%, rounded to 1.00')).toBeVisible();
	await expect(cell('DLX · 2 adults', today, '173.00')).toBeVisible();
	await page.getByLabel('Rate plan').selectOption({ label: 'BAR' });

	// Fill next month at 100.00, then weekends +10%, each previewed before it is applied.
	const month = nextMonth(today);
	const bulk = page.getByRole('dialog', { name: 'Bulk change' });
	await page.getByRole('button', { name: 'Bulk change…' }).click();
	await bulk.getByLabel('From').fill(month.start);
	await bulk.getByLabel('Through').fill(month.end);
	await bulk.getByLabel('Change').selectOption('set');
	await bulk.getByLabel('Value').fill('100');
	await bulk.getByRole('button', { name: 'Preview' }).click();
	await expect(bulk.getByTestId('preview-total')).toContainText('prices change');
	await bulk.getByRole('button', { name: 'Apply' }).click();
	await expect(bulk).toBeHidden();
	await page.getByRole('button', { name: 'Bulk change…' }).click();
	await bulk.getByLabel('From').fill(month.start);
	await bulk.getByLabel('Through').fill(month.end);
	for (const day of ['Mon', 'Tue', 'Wed', 'Thu', 'Fri']) await bulk.getByLabel(day).uncheck();
	await bulk.getByLabel('Change').selectOption('percent');
	await bulk.getByLabel('Value').fill('10');
	await bulk.getByRole('button', { name: 'Preview' }).click();
	await expect(bulk.getByRole('table', { name: 'Changes' })).toContainText('100.00 → 110.00');
	await bulk.getByRole('button', { name: 'Apply' }).click();
	await expect(bulk).toBeHidden();

	await page.getByRole('button', { name: 'Next month' }).click();
	await expect(cell('DLX · 2 adults', month.saturday, '110.00')).toBeVisible();
	await expect(cell('DLX · 1 adult', month.monday, '100.00')).toBeVisible();
	await page.getByLabel('Rate plan').selectOption({ label: 'OTA' });
	await expect(cell('DLX · 2 adults', month.saturday, '127.00')).toBeVisible();
	await expect(cell('DLX · 2 adults', month.monday, '115.00')).toBeVisible();
	await page.getByLabel('Rate plan').selectOption({ label: 'BAR' });
	await page.getByRole('button', { name: 'Previous month' }).click();

	// A minimum stay shows on the restrictions row and stops a one-night quote.
	const restrictions = page.getByRole('dialog', { name: 'Restrictions' });
	await page.getByRole('button', { name: 'Restrictions…' }).click();
	await restrictions.getByLabel('From').fill(today);
	await restrictions.getByLabel('Through').fill(today);
	await restrictions.getByLabel('Minimum stay').fill('2');
	await restrictions.getByRole('button', { name: 'Save restrictions' }).click();
	await expect(restrictions).toBeHidden();
	await expect(cell('DLX · restrictions', today, 'Min 2')).toBeVisible();

	const quote = page.getByRole('form', { name: 'Quote' });
	await quote.getByLabel('Check-in').fill(today);
	await quote.getByLabel('Check-out').fill(addDays(today, 1));
	await quote.getByLabel('Residency').selectOption('NON_RESIDENT');
	await quote.getByRole('button', { name: 'Quote' }).click();
	const result = page.getByRole('region', { name: 'Quote result' });
	await expect(result).toContainText('Total 150.00 USD');
	await expect(result).toContainText(`stays over ${today} are at least 2 nights`);
});
