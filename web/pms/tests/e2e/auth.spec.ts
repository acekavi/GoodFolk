import { expect, test } from '@playwright/test';
import { PASSWORD, signUp } from './helpers';

test('a new user signs up, signs out and signs back in', async ({ page }) => {
	const { email } = await signUp(page);

	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();

	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
});

test('repeated wrong passwords lock sign-in for the email', async ({ page }) => {
	const { email } = await signUp(page);
	await page.getByRole('button', { name: 'Sign out' }).click();

	for (let attempt = 1; attempt <= 5; attempt++) {
		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill('not the password');
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('alert')).toHaveText('Invalid email or password');
	}
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();

	await expect(page.getByRole('alert')).toContainText('too many failed sign-in attempts');
});
