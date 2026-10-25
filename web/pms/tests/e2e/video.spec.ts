import { expect, test, type Page, type Request } from '@playwright/test';
import { bookableHotel, createProperty, runsQuery, signUp } from './helpers';

// Room tour videos end to end: the file goes straight to RustFS, media-svc makes the renditions and the
// poster, and its result refreshes the gallery live. Needs RustFS on :9000 and ffmpeg (see media.spec.ts).

const VIDEO = 'tests/e2e/fixtures/tour.mp4';
/** Processing runs in media-svc's workers; a short clip takes a few seconds. */
const READY = { timeout: 60_000 };
const GALLERY = 'section[aria-label="Photos of DLX"]';

/** A signed-up owner's property with a DLX room type, and the page on its room types. */
async function roomTypes(page: Page, code: string) {
	await signUp(page);
	await createProperty(page, code);
	const propertyId = page.url().split('/p/')[1];
	await bookableHotel(page, 2, 1);
	await page.goto(`/p/${propertyId}/room-types`);
	return page.getByRole('region', { name: 'Photos of DLX' });
}

test('a room tour video is uploaded, goes ready live, plays with sound and is removed', async ({
	page
}) => {
	const gallery = await roomTypes(page, 'VID');
	await expect(page.getByRole('heading', { name: 'Photos and videos' })).toBeVisible();

	// Every state the gallery shows is recorded inside the page, so a short "Processing…" is not missed, and
	// a mark on the window shows the page is never reloaded on the way to ready.
	await page.evaluate((selector) => {
		const seen: string[] = [];
		Object.assign(window, { seen, unreloaded: true });
		const section = document.querySelector(selector)!;
		new MutationObserver(() => {
			for (const state of section.querySelectorAll('.state')) seen.push(state.textContent!.trim());
		}).observe(section, { subtree: true, childList: true, characterData: true });
	}, GALLERY);
	await gallery.locator('input[type="file"]').setInputFiles(VIDEO);

	const play = gallery.getByRole('button', { name: 'Play video, 0:04' });
	await expect(play).toBeVisible(READY);
	await expect(play).toContainText('0:04');
	const { seen, unreloaded } = await page.evaluate(() => {
		const marked = window as unknown as { seen: string[]; unreloaded?: boolean };
		return { seen: marked.seen, unreloaded: marked.unreloaded };
	});
	expect(seen).toContain('Processing…');
	expect(unreloaded).toBe(true);
	await expect(gallery.getByText('Processing…')).toHaveCount(0);
	await expect(gallery.locator('video')).toHaveCount(0);

	// A tap opens the player in place and plays it with sound.
	await play.click();
	await expect(gallery.locator('video')).toHaveCount(1);
	await expect(gallery.locator('video')).toBeFocused();
	await page.waitForFunction(() => {
		const v = document.querySelector('video');
		return !!v && !v.paused && v.currentTime > 0.5;
	});
	const playing = await page.evaluate(() => {
		const v = document.querySelector('video')!;
		const types = [...v.querySelectorAll('source')].map((source) => source.type);
		return { muted: v.muted, types };
	});
	expect(playing.muted).toBe(false);
	// The clip is smaller than 720p, which is never upscaled: AV1 and H.264, AV1 first.
	expect(playing.types).toHaveLength(2);
	expect(playing.types[0]).toContain('av01');
	expect(playing.types[1]).toContain('avc1');

	await gallery.getByRole('button', { name: 'Remove video 1' }).click();
	await gallery.getByRole('button', { name: 'Remove video', exact: true }).click();
	await expect(gallery.locator('video')).toHaveCount(0);
	await page.reload();
	await expect(gallery).toContainText('No photos yet.');
	await expect(gallery.getByRole('button', { name: /^Play video/ })).toHaveCount(0);
});

test('a fourth video is refused in the browser when three are listed', async ({ page }) => {
	// Three ready videos are listed: the gallery's answer is stubbed rather than waiting for three transcodes.
	await page.route('**/graphql', async (route) => {
		if (!runsQuery(route.request(), 'RoomTypeGalleries')) return route.fallback();
		const response = await route.fetch();
		const body = await response.json();
		for (const type of body.data.roomTypes) {
			type.media = [1, 2, 3].map((n) => ({
				assetId: `00000000-0000-4000-8000-00000000000${n}`,
				status: 'READY',
				error: null,
				caption: null,
				width: 640,
				height: 360,
				kind: 'VIDEO',
				durationMs: 4000,
				sources: [],
				renditions: []
			}));
		}
		await route.fulfill({ response, json: body });
	});
	const started: Request[] = [];
	page.on('request', (request) => {
		if (request.url().endsWith('/media/uploads')) started.push(request);
	});
	const gallery = await roomTypes(page, 'FOUR');
	await expect(gallery.getByRole('button', { name: 'Play video, 0:04' })).toHaveCount(3);

	await gallery.locator('input[type="file"]').setInputFiles({
		name: 'fourth.mp4',
		mimeType: 'video/mp4',
		buffer: Buffer.alloc(1024)
	});
	await expect(gallery.getByLabel('Upload of fourth.mp4')).toContainText(
		'A room type can have at most 3 videos'
	);
	expect(started).toHaveLength(0);
});
