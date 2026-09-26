import { expect, test } from '@playwright/test';
import { addRooms, addRoomType, createProperty, signUp } from './helpers';

/** `YYYY-MM-DD` plus `days`. */
function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

test('blocking a room reduces availability on the calendar until it is released', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Room types' }).click();
	await addRoomType(page, 'DLX', 'Deluxe');
	await page.getByRole('link', { name: 'Rooms', exact: true }).click();
	await addRooms(page, 'DLX', 101, 105);

	await page.getByRole('link', { name: 'Inventory' }).click();
	const today = (await page.getByTestId('business-date').textContent())!.trim();
	const grid = page.getByRole('grid', { name: 'Availability' });
	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })).toBeVisible();

	await page.getByRole('button', { name: 'Block a room' }).click();
	const dialog = page.getByRole('dialog', { name: 'Block a room' });
	await dialog.getByLabel('Room').selectOption({ label: '101 · DLX' });
	await dialog.getByLabel('From').fill(today);
	await dialog.getByLabel('Until (first day back)').fill(addDays(today, 2));
	await dialog.getByLabel('Reason').selectOption({ label: 'Maintenance' });
	await dialog.getByLabel('Note').fill('Leaking pipe');
	await dialog.getByRole('button', { name: 'Block room' }).click();
	await expect(dialog).toBeHidden();

	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 4 available` })).toBeVisible();
	await expect(
		grid.getByRole('gridcell', { name: `DLX ${addDays(today, 1)}: 4 available` })
	).toBeVisible();

	// A second block over the same days is refused and names the block in the way.
	await page.getByRole('button', { name: 'Block a room' }).click();
	await dialog.getByLabel('Room').selectOption({ label: '101 · DLX' });
	await dialog.getByLabel('From').fill(addDays(today, 1));
	await dialog.getByLabel('Until (first day back)').fill(addDays(today, 3));
	await dialog.getByLabel('Reason').selectOption({ label: 'Renovation' });
	await dialog.getByRole('button', { name: 'Block room' }).click();
	await expect(dialog.getByRole('alert')).toContainText(
		`Room 101 is already blocked from ${today} until ${addDays(today, 2)}`
	);
	await dialog.getByRole('button', { name: 'Cancel' }).click();

	// Click a cell and press ArrowRight: the active cell should move one column, not two.
	const cellToday = grid.getByRole('gridcell', { name: `DLX ${today}: 4 available` });
	const cellTomorrow = grid.getByRole('gridcell', {
		name: `DLX ${addDays(today, 1)}: 4 available`
	});
	await cellToday.click();
	await page.keyboard.press('ArrowRight');
	const tomorrowId = await cellTomorrow.getAttribute('id');
	expect(tomorrowId).not.toBeNull();
	await expect(grid).toHaveAttribute('aria-activedescendant', tomorrowId ?? '');

	// Keyboard: go to the first day of the month, walk right to the business date and open it.
	await grid.focus();
	await page.keyboard.press('Home');
	for (let day = 1; day < Number(today.slice(8)); day++) await page.keyboard.press('ArrowRight');
	await page.keyboard.press('Enter');
	const day = page.getByRole('region', { name: `Blocks on ${today}` });
	// The day's blocks are announced as they appear, and focus stays on the grid.
	await expect(day).toHaveAttribute('aria-live', 'polite');
	await expect(grid).toBeFocused();
	await expect(day).toContainText('101');
	await expect(day).toContainText('Leaking pipe');
	await day.getByRole('button', { name: 'Release room 101' }).click();

	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })).toBeVisible();
});

test('the active cell stays on its row when room types are retired and restored', async ({
	page,
	context
}) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Room types' }).click();
	for (const code of ['AAA', 'BBB', 'CCC']) await addRoomType(page, code, `Type ${code}`);
	const settings = await context.newPage();
	await settings.goto(page.url());

	await page.getByRole('link', { name: 'Inventory' }).click();
	const today = (await page.getByTestId('business-date').textContent())!.trim();
	const grid = page.getByRole('grid', { name: 'Availability' });
	const cell = (code: string) => grid.getByRole('gridcell', { name: `${code} ${today}:` });
	await cell('CCC').click();
	await expect(grid).toHaveAttribute(
		'aria-activedescendant',
		(await cell('CCC').getAttribute('id'))!
	);

	// Retiring the last type moves the active cell up a row; restoring it must not move it back.
	await settings.getByRole('button', { name: 'Deactivate CCC' }).click();
	await expect(cell('CCC')).toHaveCount(0);
	const bbb = (await cell('BBB').getAttribute('id'))!;
	await expect(grid).toHaveAttribute('aria-activedescendant', bbb);
	await settings.getByRole('button', { name: 'Activate CCC' }).click();
	await expect(cell('CCC')).toBeVisible();
	await expect(grid).toHaveAttribute('aria-activedescendant', bbb);
});
