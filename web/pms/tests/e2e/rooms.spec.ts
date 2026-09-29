import { expect, test } from '@playwright/test';
import { addRooms, addRoomType, createProperty, signUp } from './helpers';

test('an owner sets up room types and rooms', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GAL');

	await page.getByRole('link', { name: 'Room types' }).click();
	await addRoomType(page, 'STD', 'Standard');
	await addRoomType(page, 'DLX', 'Deluxe');
	await page.getByRole('button', { name: 'Move DLX up' }).click();
	await expect(page.getByRole('row').nth(1)).toContainText('DLX');
	await page.getByRole('button', { name: 'Edit DLX' }).click();
	await page.getByLabel('Name of DLX').fill('Deluxe Sea View');
	await page.getByLabel('Overbooking allowance of DLX').fill('5');
	await page.getByRole('button', { name: 'Save DLX' }).click();
	await expect(page.getByRole('cell', { name: 'Deluxe Sea View' })).toBeVisible();
	const dlxRow = page.getByRole('row', { name: /Deluxe Sea View/ });
	await expect(dlxRow.getByRole('cell', { name: '5', exact: true })).toBeVisible();

	await page.getByRole('link', { name: 'Rooms', exact: true }).click();
	await addRooms(page, 'DLX', 101, 105);
	await addRooms(page, 'STD', 201, 202);
	await expect(page.getByRole('heading', { name: 'DLX · Deluxe Sea View' })).toBeVisible();

	await page.getByLabel('Group by').selectOption('floor');
	await expect(page.getByRole('heading', { name: 'Floor 1' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Floor 2' })).toBeVisible();

	await page.getByLabel('Deactivate room 105').click();
	await expect(page.getByRole('button', { name: 'Activate room 105' })).toBeVisible();
});
