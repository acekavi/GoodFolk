import { resolve } from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { bookableHotel, createProperty, runAsOwner, signUp } from './helpers';

// Phase 6 gate for galleries: a room type's gallery of 50 photos paints in under 400 ms (p90 of ten cold
// loads of the Room types page). Timings on shared CI runners are noise, so this is left out of the default
// run. Run it locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf
// The numbers depend on the CPU governor (`powersave` on the development laptop) and on other heavy jobs, so
// record both with a result. The gate is read inside the page with `performance.now()`, as in tape-perf.spec.ts.
//
// The photos are seeded as ready, straight into the database (as the schema owner) and RustFS's public
// bucket, so that the gate times the gallery and not media-svc. Their renditions in fixtures/gallery are
// what media-svc makes of fixtures/small.jpg (800x600):
//   cargo run --release -p imaging --example renditions -- crates/imaging/tests/fixtures/small.jpg \
//     web/pms/tests/e2e/fixtures/gallery

// The trace's DOM snapshots run in the page beside the load being timed and add tens of milliseconds to it.
test.use({ trace: 'off' });

const PHOTOS = 50;
const GALLERY = 'section[aria-label="Photos of DLX"]';
/** Each rendition of the fixture photo: its size, format and pixels. */
const RENDITIONS = [
	{ size: 'thumb', format: 'avif', width: 320, height: 320 },
	{ size: 'thumb', format: 'webp', width: 320, height: 320 },
	{ size: 'sm', format: 'avif', width: 640, height: 480 },
	{ size: 'sm', format: 'webp', width: 640, height: 480 }
];

/**
 * Adds `PHOTOS` ready photos to the room type's gallery: each a `media_asset` row with its renditions and a
 * `room_type_media` row, in the tenant's scope as row-level security requires, and its rendition files in
 * the public bucket, PUT as media-svc stores them (with its Cache-Control) at presigned URLs.
 */
function seedGallery(tenant: string, property: string, roomType: string) {
	runAsOwner(
		`
		const { readFileSync } = require('node:fs');
		const { TENANT, PROPERTY, ROOM_TYPE, FIXTURES, PHOTOS } = process.env;
		const renditions = JSON.parse(process.env.RENDITIONS);
		const files = renditions.map((r) => readFileSync(\`\${FIXTURES}/\${r.size}.\${r.format}\`));
		// RustFS's local development credentials (deploy/dev/rustfs.sh).
		const s3 = new Bun.S3Client({
			accessKeyId: 'goodfolk',
			secretAccessKey: 'goodfolk_dev_secret',
			endpoint: 'http://localhost:9000',
			bucket: 'goodfolk-public',
			region: 'us-east-1'
		});
		const sql = new Bun.SQL(process.env.URL);
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${TENANT}, true)\`;
			for (let position = 0; position < Number(PHOTOS); position++) {
				const asset = crypto.randomUUID();
				const stored = renditions.map((r, i) => ({
					...r,
					bytes: files[i].length,
					key: \`t/\${TENANT}/a/\${asset}/\${r.size}.\${r.format}\`
				}));
				for (const [i, r] of stored.entries()) {
					const response = await fetch(s3.presign(r.key, { method: 'PUT', expiresIn: 600 }), {
						method: 'PUT',
						body: files[i],
						headers: {
							'content-type': \`image/\${r.format}\`,
							'cache-control': 'public, max-age=31536000, immutable'
						}
					});
					if (!response.ok) throw new Error(\`PUT \${r.key}: \${response.status}\`);
				}
				const digest = () => Buffer.from(crypto.getRandomValues(new Uint8Array(32)));
				await tx\`insert into media_asset (id, tenant_id, property_id, purpose, visibility, status, filename,
						declared_bytes, sha256, blake3, mime, bytes, width, height, renditions, processing_at, ready_at)
					values (\${asset}, \${TENANT}, \${PROPERTY}, 'gallery', 'public', 'ready', \${\`photo-\${position + 1}.jpg\`},
						129929, \${digest()}, \${digest()}, 'image/jpeg', 129929, 800, 600, \${JSON.stringify(stored)}::text::jsonb, now(), now())\`;
				await tx\`insert into room_type_media (tenant_id, room_type_id, asset_id, position)
					values (\${TENANT}, \${ROOM_TYPE}, \${asset}, \${position})\`;
			}
		});
		await sql.close();
	`,
		{
			TENANT: tenant,
			PROPERTY: property,
			ROOM_TYPE: roomType,
			FIXTURES: resolve('tests/e2e/fixtures/gallery'),
			PHOTOS: String(PHOTOS),
			RENDITIONS: JSON.stringify(RENDITIONS)
		}
	);
}

const sorted = (values: number[]) => [...values].sort((a, b) => a - b);
const percentile = (values: number[], p: number) =>
	sorted(values)[Math.ceil(p * values.length) - 1];
const median = (values: number[]) => sorted(values)[Math.floor(values.length / 2)];

let page: Page;
let propertyId: string;

test.beforeAll(async ({ browser }) => {
	test.setTimeout(300_000);
	page = await browser.newPage();
	await signUp(page);
	await createProperty(page, 'MPF');
	propertyId = page.url().split('/p/')[1];
	const hotel = await bookableHotel(page, 1, 1);
	const me = await (await page.request.get('/api/v1/me')).json();
	seedGallery(me.current_tenant, propertyId, hotel.roomTypeId);

	// What the API serves, not what was meant: 50 ready photos, whose files storage serves as media-svc's are.
	const response = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ roomTypes(propertyId: "${propertyId}") { media { status renditions { size format url } } } }`
		}
	});
	const { data, errors } = await response.json();
	expect(errors).toBeUndefined();
	const media: { status: string; renditions: { size: string; format: string; url: string }[] }[] =
		data.roomTypes[0].media;
	expect(media).toHaveLength(PHOTOS);
	expect(media.every((item) => item.status === 'READY' && item.renditions.length === 4)).toBe(true);
	const file = await page.request.get(
		media[0].renditions.find((r) => r.size === 'THUMB' && r.format === 'AVIF')!.url
	);
	expect(file.status()).toBe(200);
	expect(file.headers()['content-type']).toBe('image/avif');
	expect(file.headers()['cache-control']).toBe('public, max-age=31536000, immutable');
});

test.afterAll(async () => {
	await page.close();
});

test('a gallery of 50 photos paints under 400 ms at p90 over ten cold loads @perf', async ({
	browser
}) => {
	const session = await page.context().storageState();
	const timings: number[] = [];
	const shown: number[] = [];
	for (let run = 0; run < 10; run++) {
		// A new context each time: nothing cached, as on a first visit.
		const context = await browser.newContext({ storageState: session });
		const fresh = await context.newPage();
		// From navigation to the frame after the gallery holds all its photos and every one of them in view has
		// loaded and decoded, then a task after that frame has painted.
		await fresh.addInitScript(
			({ gallery, photos }) => {
				(window as unknown as { galleryPaint: Promise<object> }).galleryPaint = new Promise(
					(resolve) => {
						const observer = new MutationObserver(() => {
							const images = [
								...document.querySelectorAll<HTMLImageElement>(`${gallery} .tiles img`)
							];
							if (images.length !== photos) return;
							observer.disconnect();
							const inView = images.filter((img) => {
								const box = img.getBoundingClientRect();
								return (
									box.bottom > 0 &&
									box.top < window.innerHeight &&
									box.right > 0 &&
									box.left < window.innerWidth
								);
							});
							void Promise.all(inView.map((img) => img.decode().catch(() => {}))).then(() =>
								requestAnimationFrame(() => {
									const channel = new MessageChannel();
									channel.port1.onmessage = () =>
										resolve({
											ms: performance.now(),
											inView: inView.length,
											loaded: inView.filter((img) => img.complete && img.naturalWidth > 0).length
										});
									channel.port2.postMessage(null);
								})
							);
						});
						observer.observe(document, { subtree: true, childList: true, attributes: true });
					}
				);
			},
			{ gallery: GALLERY, photos: PHOTOS }
		);
		await fresh.goto(`/p/${propertyId}/room-types`);
		const { ms, inView, loaded } = await fresh.evaluate(
			() =>
				(
					window as never as {
						galleryPaint: Promise<{ ms: number; inView: number; loaded: number }>;
					}
				).galleryPaint
		);
		// Something really painted: photos in view, every one of them loaded from storage.
		expect(inView).toBeGreaterThan(0);
		expect(loaded).toBe(inView);
		timings.push(ms);
		shown.push(inView);
		await context.close();
	}
	console.log(
		`gallery of ${PHOTOS} first paint: ${timings.map((ms) => ms.toFixed(0)).join(', ')} ms, ` +
			`median ${median(timings).toFixed(0)}, p90 ${percentile(timings, 0.9).toFixed(0)}, ` +
			`max ${Math.max(...timings).toFixed(0)}; photos in view ${shown.join(', ')}`
	);
	expect(percentile(timings, 0.9)).toBeLessThan(400);
});
