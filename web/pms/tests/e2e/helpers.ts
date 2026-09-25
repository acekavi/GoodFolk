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
