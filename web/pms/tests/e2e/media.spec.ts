import { readFileSync } from 'node:fs';
import { expect, test, type Browser, type Page, type Request } from '@playwright/test';
import {
	addDays,
	addStaff,
	bookableHotel,
	createProperty,
	post,
	runAsOwner,
	runsQuery,
	signIn,
	signUp,
	type Hotel
} from './helpers';

// The media engine end to end: files go from the browser straight to RustFS, media-svc makes the
// renditions, and its result comes back to core-api, whose `media:<property>` event refreshes the page.
// Needs RustFS on :9000 (global-setup.ts checks it); Playwright starts media-svc.

const PHOTO = 'tests/e2e/fixtures/small.jpg';
const OTHER_PHOTO = 'tests/e2e/fixtures/alpha.png';
const PDF = 'tests/e2e/fixtures/doc.pdf';
const MIB = 1024 * 1024;
/** Uploads PUT their parts straight to RustFS, at presigned URLs. */
const STORAGE = 'http://localhost:9000/**';
/** Processing runs in media-svc's workers, so a photo takes a moment to be ready. */
const READY = { timeout: 30_000 };

/** A signed-up owner's property, bookable over the REST API, with the page left on it. */
async function hotel(page: Page, code: string) {
	const { email } = await signUp(page);
	await createProperty(page, code);
	const propertyId = page.url().split('/p/')[1];
	return { email, propertyId, ...(await bookableHotel(page, 2, 1)) };
}

/** Books one DLX room for the business date's night for the hotel's guest, Ada Silva. */
async function bookOne(page: Page, hotel: Hotel) {
	const created = await post(page.request, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source: 'front_desk',
		rooms: [
			{
				room_type_id: hotel.roomTypeId,
				rate_plan_id: hotel.ratePlanId,
				meal_plan: 'RO',
				check_in: hotel.businessDate,
				check_out: addDays(hotel.businessDate, 1),
				adults: 2
			}
		]
	});
	return { id: created.id as string, confirmation: created.confirmation_no as string };
}

/** A page of `email`, signed in, in a context of its own. */
async function signedIn(
	browser: Browser,
	email: string,
	viewport?: { width: number; height: number }
) {
	const context = await browser.newContext(viewport ? { viewport } : {});
	const page = await context.newPage();
	await signIn(page, email);
	return { page, context };
}

/** Whether every image `selector` matches has loaded from storage, and there is at least one. */
function imagesLoaded(page: Page, selector: string) {
	return page.locator(selector).evaluateAll(
		(images) =>
			images.length > 0 &&
			images.every((image) => {
				const img = image as HTMLImageElement;
				return img.complete && img.naturalWidth > 0;
			})
	);
}

const GALLERY = 'section[aria-label="Photos of DLX"]';

/** The `src` of each photo in the DLX gallery, in order: each names its asset. */
function gallerySources(page: Page) {
	return page
		.locator(`${GALLERY} .tiles img`)
		.evaluateAll((images) => images.map((img) => img.getAttribute('src')));
}

test('a gallery upload goes from processing to ready live, then a reorder survives a reload and the caption is saved', async ({
	page
}) => {
	const { propertyId } = await hotel(page, 'GAL');
	await page.goto(`/p/${propertyId}/room-types`);
	const gallery = page.getByRole('region', { name: 'Photos of DLX' });
	await expect(gallery).toContainText('No photos yet.');

	// Every state the gallery shows is recorded inside the page, so a short "Processing…" is not missed, and a
	// mark on the window shows the page is never reloaded on the way to ready.
	await page.evaluate((selector) => {
		const seen: string[] = [];
		Object.assign(window, { seen, unreloaded: true });
		const section = document.querySelector(selector)!;
		new MutationObserver(() => {
			for (const state of section.querySelectorAll('.state')) seen.push(state.textContent!.trim());
		}).observe(section, { subtree: true, childList: true, characterData: true });
	}, GALLERY);
	// Two different photos, so their order can be told apart.
	await gallery.locator('input[type="file"]').setInputFiles([PHOTO, OTHER_PHOTO]);

	await expect(gallery.getByRole('img')).toHaveCount(2, READY);
	await expect.poll(() => imagesLoaded(page, `${GALLERY} .tiles img`)).toBe(true);
	const { seen, unreloaded } = await page.evaluate(() => {
		const marked = window as unknown as { seen: string[]; unreloaded?: boolean };
		return { seen: marked.seen, unreloaded: marked.unreloaded };
	});
	expect(seen).toContain('Processing…');
	expect(unreloaded).toBe(true);
	await expect(gallery.getByText('Processing…')).toHaveCount(0);

	// Move the first photo later; the order survives a reload.
	const before = await gallerySources(page);
	expect(new Set(before).size).toBe(2);
	const ordered = page.waitForResponse(
		(response) => response.url().endsWith('/media/order') && response.request().method() === 'PUT'
	);
	await gallery.getByRole('button', { name: 'Move photo 1 later' }).click();
	expect((await ordered).ok()).toBe(true);
	await page.reload();
	await expect(gallery.getByRole('img')).toHaveCount(2);
	expect(await gallerySources(page)).toEqual([before[1], before[0]]);

	// A caption is saved, survives a reload, and becomes the photo's text alternative.
	await gallery.getByRole('button', { name: 'Edit caption of photo 1' }).click();
	await gallery.getByLabel('Caption of photo 1').fill('Garden view from the balcony');
	await gallery.getByRole('button', { name: 'Save' }).click();
	await expect(gallery.getByText('Garden view from the balcony')).toBeVisible();
	await page.reload();
	await expect(gallery.getByText('Garden view from the balcony')).toBeVisible();
	await expect(gallery.getByRole('img', { name: 'Garden view from the balcony' })).toHaveAttribute(
		'src',
		before[1]!
	);
});

test('a housekeeper adds a photo to an issue from /hk, and leaving while it uploads asks first', async ({
	page,
	browser
}) => {
	const { email, propertyId } = await hotel(page, 'ISS');
	const { email: housekeeper } = addStaff(email, 'housekeeping', 'Kumari Silva');
	const { page: hk, context } = await signedIn(browser, housekeeper, { width: 375, height: 700 });

	// The photo's part is held at storage until released, so the upload stays in flight.
	let release!: () => void;
	const released = new Promise<void>((resolve) => (release = resolve));
	let holding!: () => void;
	const held = new Promise<void>((resolve) => (holding = resolve));
	await hk.route(STORAGE, async (route) => {
		if (route.request().method() !== 'PUT') return route.fallback();
		holding();
		await released;
		await route.continue();
	});

	await hk.goto(`/hk/issue?property=${propertyId}`);
	const form = hk.getByRole('form', { name: 'Report an issue' });
	await form.getByLabel('Room').selectOption({ label: '101' });
	await form.getByLabel('Description').fill('Cracked mirror');
	await form.locator('input[type="file"]').setInputFiles(PHOTO);
	await expect(form.getByRole('list', { name: 'Chosen photos' })).toContainText('small.jpg');
	await form.getByRole('button', { name: 'Report issue' }).click();
	await expect(hk.getByRole('status')).toHaveText('Issue reported for room 101.');
	const photos = hk.getByRole('region', { name: 'Photos of the issue in room 101' });
	await expect(photos.getByRole('group', { name: 'Upload of small.jpg' })).toBeVisible();
	await held;

	// Leaving now would drop the photo: the app asks, and dismissing keeps the page.
	const asked: string[] = [];
	hk.once('dialog', (dialog) => {
		asked.push(dialog.message());
		void dialog.dismiss();
	});
	await hk.getByRole('link', { name: 'My rooms' }).click();
	await expect.poll(() => asked).toEqual(['Photos are still uploading. Leave and drop them?']);
	await expect(hk).toHaveURL(/\/hk\/issue\?/);
	await expect(hk.getByRole('heading', { name: 'Report issue' })).toBeVisible();

	release();
	await expect(photos.getByRole('button', { name: 'Open photo 1' })).toBeVisible(READY);
	await expect
		.poll(() => imagesLoaded(hk, 'section[aria-label="Photos of the issue in room 101"] img'))
		.toBe(true);
	// Nothing is uploading any more, so leaving does not ask (an unexpected prompt would be dismissed and
	// keep the page).
	await hk.getByRole('link', { name: 'My rooms' }).click();
	await expect(hk.getByRole('heading', { name: 'My rooms' })).toBeVisible();
	expect(asked).toHaveLength(1);

	// The owner's board shows the photo on the open issue.
	await page.goto(`/p/${propertyId}/housekeeping`);
	const issue = page.getByRole('list', { name: 'Open issues' }).getByRole('listitem');
	await expect(issue).toContainText('Cracked mirror');
	await expect(issue.getByRole('button', { name: 'Open photo 1' })).toBeVisible();
	await context.close();
});

/** The tenant of the signed-in user. */
async function tenantOf(page: Page): Promise<string> {
	const me = await page.request.get('/api/v1/me');
	expect(me.status()).toBe(200);
	return (await me.json()).current_tenant;
}

test('a guest ID is uploaded and viewed: one audited link per view, and closing clears the image', async ({
	page
}) => {
	const owner = await hotel(page, 'GID');
	const reservation = await bookOne(page, owner);
	const links: Request[] = [];
	page.on('request', (request) => {
		if (/\/guest-documents\/[^/]+\/link/.test(request.url())) links.push(request);
	});

	await page.goto(`/p/${owner.propertyId}/reservations/${reservation.id}`);
	const modal = page.getByRole('dialog', { name: `${reservation.confirmation} · Confirmed` });
	await modal.getByText('ID documents · Ada Silva').click();
	const documents = modal.getByRole('region', { name: 'ID documents of Ada Silva' });
	await expect(documents).toContainText('No ID documents on file.');
	await documents.getByLabel('Kind').selectOption('passport');
	await documents.locator('input[type="file"]').setInputFiles(PHOTO);

	const view = documents.getByRole('button', { name: /^View Passport from / });
	await expect(view).toBeVisible(READY);
	await expect(documents.getByRole('list', { name: 'ID documents' })).toContainText('Nimal Perera');
	expect(links).toHaveLength(0);

	const viewer = page.getByRole('dialog', { name: 'Passport', exact: true });
	const image = viewer.getByRole('img', { name: 'Passport of Ada Silva' });
	for (let views = 1; views <= 2; views++) {
		await view.click();
		await expect(image).toBeVisible();
		await expect.poll(() => imagesLoaded(page, 'dialog img')).toBe(true);
		expect(links).toHaveLength(views);
		await viewer.getByRole('button', { name: 'Close' }).click();
		await expect(viewer).toBeHidden();
		// The link is not kept: the closed dialog holds no image.
		await expect(page.locator('dialog img')).toHaveCount(0);
		await expect(modal).toBeVisible();
	}

	// Each view wrote its audit entry.
	const audited = runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		const [{ count }] = await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			return tx\`select count(*)::int as count from audit_log
				where tenant_id = \${process.env.TENANT} and action = 'guest_document.viewed'\`;
		});
		await sql.close();
		// A string, which is never coloured, even under FORCE_COLOR.
		console.log(String(count));
	`,
		{ TENANT: await tenantOf(page) }
	);
	expect(Number(audited)).toBe(2);
});

test('a housekeeper sees no guest documents and never asks for them', async ({ page, browser }) => {
	const owner = await hotel(page, 'GHK');
	const reservation = await bookOne(page, owner);
	const { email: housekeeper } = addStaff(owner.email, 'housekeeping', 'Kumari Silva');
	const { page: hk, context } = await signedIn(browser, housekeeper);
	const asked: Request[] = [];
	hk.on('request', (request) => {
		if (request.url().endsWith('/graphql') && runsQuery(request, 'GuestDocuments'))
			asked.push(request);
	});

	// Who may see what is known once the profile has loaded; the owner's test above shows the section.
	const profile = hk.waitForResponse((response) => response.url().endsWith('/api/v1/me'));
	await hk.goto(`/p/${owner.propertyId}/reservations/${reservation.id}`);
	expect((await profile).ok()).toBe(true);
	const modal = hk.getByRole('dialog', { name: `${reservation.confirmation} · Confirmed` });
	await expect(modal).toContainText('Ada Silva');
	await expect(modal.getByRole('table', { name: 'History' })).toBeVisible();
	await expect(modal.getByText(/ID documents/)).toHaveCount(0);
	expect(asked).toHaveLength(0);

	// And the API refuses them anyway.
	const response = await hk.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ guestDocuments(propertyId: "${owner.propertyId}", guestId: "${owner.guestId}") { id } }`
		}
	});
	const { data, errors } = await response.json();
	expect(data?.guestDocuments ?? null).toBeNull();
	expect(errors?.length).toBeGreaterThan(0);
	await context.close();
});

test('an oversized file and a file of the wrong type are refused with their reasons', async ({
	page
}) => {
	const { propertyId } = await hotel(page, 'BIG');
	const started: Request[] = [];
	page.on('request', (request) => {
		if (request.url().endsWith('/media/uploads')) started.push(request);
	});
	await page.goto(`/p/${propertyId}/room-types`);
	const gallery = page.getByRole('region', { name: 'Photos of DLX' });
	await gallery.locator('input[type="file"]').setInputFiles([
		{ name: 'huge.jpg', mimeType: 'image/jpeg', buffer: Buffer.alloc(25 * MIB + 1) },
		{ name: 'doc.pdf', mimeType: 'application/pdf', buffer: readFileSync(PDF) }
	]);
	await expect(gallery.getByLabel('Upload of huge.jpg')).toContainText(
		'Too large: the limit is 25 MB'
	);
	await expect(gallery.getByLabel('Upload of doc.pdf')).toContainText('Not a supported image');
	await expect(gallery.getByRole('alert')).toHaveText([
		'Too large: the limit is 25 MB',
		'Not a supported image'
	]);
	// Refused in the browser: nothing was sent.
	expect(started).toHaveLength(0);
});
