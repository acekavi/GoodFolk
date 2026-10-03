import { expect, test } from '@playwright/test';
import { housekeepingProperty, signIn } from './helpers';

// Phase 5 gate: after install, a reload offline shows My rooms. Asserted, not timed, so it runs with the default
// suite, on a small property.

test('after install, a reload offline shows My rooms', async ({ browser }) => {
	test.setTimeout(120_000);
	const owner = await browser.newPage();
	const { propertyId, housekeeper } = await housekeepingProperty(owner, 'HKO', 4, 2);
	await owner.close();
	const context = await browser.newContext({ viewport: { width: 375, height: 700 } });
	const page = await context.newPage();
	await signIn(page, housekeeper);
	await page.goto(`/hk?property=${propertyId}`);
	const rooms = page.getByRole('list', { name: 'Rooms' }).getByRole('listitem');
	await expect(rooms).toHaveCount(2);
	await page.evaluate(() => navigator.serviceWorker.ready.then(() => undefined));
	// Let the saved list reach IndexedDB before the connection goes.
	await expect
		.poll(() =>
			page.evaluate(
				() =>
					new Promise<boolean>((done) => {
						const open = indexedDB.open('goodfolk-hk');
						open.onsuccess = () => {
							const read = open.result.transaction('kv').objectStore('kv').getAllKeys();
							read.onsuccess = () =>
								done(read.result.some((key) => String(key).startsWith('last:')));
							read.onerror = () => done(false);
						};
						open.onerror = () => done(false);
					})
			)
		)
		.toBe(true);

	await context.setOffline(true);
	await page.reload();
	await expect(page.getByRole('heading', { name: 'My rooms' })).toBeVisible();
	await expect(rooms).toHaveCount(2);
	await expect(page.getByText('You are offline.')).toBeVisible();
	await context.close();
});
