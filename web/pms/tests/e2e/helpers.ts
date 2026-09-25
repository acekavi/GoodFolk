import { expect, type Page } from '@playwright/test';

export const PASSWORD = 'a long enough password';

/** Signs up a new owner with a unique email and waits for the property list. */
export async function signUp(page: Page): Promise<{ email: string }> {
	const email = `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	await page.goto('/signup');
	await page.getByLabel('Your name').fill('Nimal Perera');
	await page.getByLabel('Hotel or group name').fill('Lagoon Hotels');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Create account' }).click();
	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
	return { email };
}

/** Creates a property and waits for its page. */
export async function createProperty(page: Page, code: string): Promise<void> {
	await page.getByRole('link', { name: 'Add a property' }).click();
	await page.getByLabel('Code').fill(code);
	await page.getByLabel('Name').fill(`Hotel ${code}`);
	await page.getByRole('button', { name: 'Create property' }).click();
	await expect(page.getByRole('heading', { name: `Hotel ${code}` })).toBeVisible();
}

/** Adds a room type on the Room types page. */
export async function addRoomType(page: Page, code: string, name: string): Promise<void> {
	const form = page.getByRole('form', { name: 'New room type' });
	await form.getByLabel('Code').fill(code);
	await form.getByLabel('Name').fill(name);
	await form.getByRole('button', { name: 'Add room type' }).click();
	await expect(page.getByRole('cell', { name: code, exact: true })).toBeVisible();
}

/** Adds rooms `first` to `last` of one type with the bulk dialog on the Rooms page. */
export async function addRooms(page: Page, code: string, first: number, last: number) {
	await page.getByRole('button', { name: 'Add rooms…' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add rooms' });
	await dialog.getByLabel('Room type').selectOption({ label: code });
	await dialog.getByLabel('First number').fill(String(first));
	await dialog.getByLabel('Last number').fill(String(last));
	await dialog.getByLabel('Floor').fill(String(first).slice(0, 1));
	const count = last - first + 1;
	await dialog.getByRole('button', { name: `Add ${count} rooms` }).click();
	await expect(dialog).toBeHidden();
	await expect(page.getByRole('cell', { name: String(last), exact: true })).toBeVisible();
}
