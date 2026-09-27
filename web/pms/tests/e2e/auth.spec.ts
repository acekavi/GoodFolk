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

test('the event stream connects once per sign-in, not again on every resync', async ({ page }) => {
	const connects: string[] = [];
	page.on('request', (request) => {
		if (new URL(request.url()).pathname === '/api/v1/events') connects.push(request.url());
	});
	await signUp(page);
	// Each connect resyncs (refetching every query, the profile included); a reconnect on each profile
	// refetch would connect again and again.
	await page.waitForTimeout(2_000);
	expect(connects).toHaveLength(1);
});
