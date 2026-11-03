// Phase 9 gate: the landing and results pages reach Largest Contentful Paint in under 1.5 s on Lighthouse's mobile
// profile (simulated slow 4G on a mid-range phone) for a property with a 10-photo gallery.
//
//   E2E_DATABASE_URL=postgres://goodfolk_api:…/goodfolk_e2e E2E_OWNER_URL=postgres://goodfolk_owner:…/goodfolk_e2e \
//     bun run lighthouse
//
// It needs what `bun run test:e2e` needs (a migrated database of its own, as the API role and as the schema owner),
// RustFS (`sh deploy/dev/rustfs.sh start`) and Playwright's Chromium (`bunx playwright install chromium`), whose
// binary Lighthouse drives. It builds and starts core-api and media-svc (release) and the built site's `vite preview`,
// seeds a hotel whose Deluxe gallery holds 10 photos (the renditions media-svc makes of a 12 MP photo, stored in
// RustFS's public bucket) with the first one as the hero, runs `@lhci/cli autorun` (pinned below) with lighthouserc.json (3 runs a
// page, every one under the gate), prints each page's LCP and stops exactly the processes it started.
// PLAYWRIGHT_NO_SANDBOX=1 runs Chromium without its sandbox, as for the e2e suite.

import { chromium, type APIRequestContext } from '@playwright/test';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { addDays, runAsOwner, seedHotel } from '../tests/e2e/helpers';

const ROOT = resolve(import.meta.dir, '../../..');
const SITE = resolve(import.meta.dir, '..');
// Ports apart from the e2e suites' and the development servers'.
const API_PORT = 18380;
const MEDIA_PORT = 8390;
const SITE_PORT = 4374;
const JOBS_PORT = 8391;
const API = `http://localhost:${API_PORT}`;
const SITE_ORIGIN = `http://localhost:${SITE_PORT}`;
const PHOTOS = 10;
/** The project's limit on parallel builds: more links at once exhaust a development machine's memory. */
const CARGO_JOBS = ['-j', process.env.CARGO_BUILD_JOBS ?? '3'];
/** Pinned, so a run never resolves a newer Lighthouse CI (and a newer Lighthouse) than the gate was measured with. */
const LHCI = '@lhci/cli@0.15.1';
const PHOTO = join(ROOT, 'crates/imaging/tests/fixtures/photo-12mp.jpg');

const SERVICE_AUTH = {
	SVC_AUTH: 'secret',
	INTERNAL_SHARED_SECRET: 'lhci-internal-shared-secret-0123456789abcdef',
	EVENT_BUS: 'push'
};
// Shared by core-api and the site's server, as in production: a render counts against the guest.
const IBE_SSR_SECRET = 'lhci-ibe-ssr-secret-0123456789abcdef-01234';

const database = process.env.E2E_DATABASE_URL;
if (!database) throw new Error('set E2E_DATABASE_URL to a migrated database, as goodfolk_api');
if (!process.env.E2E_OWNER_URL)
	throw new Error('set E2E_OWNER_URL to the same database, as goodfolk_owner');

const started: Bun.Subprocess[] = [];

/** Runs `command` to the end, failing on a non-zero exit. */
function run(command: string[], cwd: string): string {
	const done = Bun.spawnSync(command, { cwd, stdout: 'pipe', stderr: 'inherit' });
	if (done.exitCode !== 0) throw new Error(`${command.join(' ')} exited with ${done.exitCode}`);
	return done.stdout.toString();
}

/** Starts `command` in the background, kept to be stopped by its own PID. */
function start(command: string[], cwd: string, env: Record<string, string>): void {
	started.push(
		Bun.spawn(command, {
			cwd,
			env: { ...process.env, ...env },
			stdout: 'ignore',
			stderr: 'inherit'
		})
	);
}

/** Waits for `url` to answer at all (any status), for at most a minute. */
async function waitFor(url: string): Promise<void> {
	for (let tries = 0; tries < 240; tries++) {
		try {
			await fetch(url);
			return;
		} catch {
			await Bun.sleep(250);
		}
	}
	throw new Error(`nothing answered at ${url}`);
}

/**
 * Adds `PHOTOS` ready photos to the room type's gallery, as media-perf.spec.ts in web/pms does: each a `media_asset`
 * row with every gallery rendition of the 12 MP fixture (written to `dir` by the imaging crate, as media-svc makes
 * them) and a `room_type_media` row, in the tenant's scope, with the files PUT into RustFS's public bucket. Returns
 * the first photo's id.
 */
function seedGallery(tenant: string, property: string, roomType: string, dir: string): string {
	const renditions = run(
		[
			'cargo',
			'run',
			'-q',
			...CARGO_JOBS,
			'--release',
			'-p',
			'imaging',
			'--example',
			'renditions',
			'--',
			PHOTO,
			dir
		],
		ROOT
	)
		.trim()
		.split('\n')
		.map((line) => {
			const [size, format, width, height] = line.split(' ');
			return { size, format, width: Number(width), height: Number(height) };
		});
	return runAsOwner(
		`
		const { readFileSync } = require('node:fs');
		const { TENANT, PROPERTY, ROOM_TYPE, DIR, PHOTOS } = process.env;
		const renditions = JSON.parse(process.env.RENDITIONS);
		const files = renditions.map((r) => readFileSync(\`\${DIR}/\${r.size}.\${r.format}\`));
		// RustFS's local development credentials (deploy/dev/rustfs.sh).
		const s3 = new Bun.S3Client({
			accessKeyId: 'goodfolk',
			secretAccessKey: 'goodfolk_dev_secret',
			endpoint: 'http://localhost:9000',
			bucket: 'goodfolk-public',
			region: 'us-east-1'
		});
		const sql = new Bun.SQL(process.env.URL);
		const ids = [];
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${TENANT}, true)\`;
			for (let position = 0; position < Number(PHOTOS); position++) {
				const asset = crypto.randomUUID();
				ids.push(asset);
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
						4571573, \${digest()}, \${digest()}, 'image/jpeg', 4571573, 4000, 3000, \${JSON.stringify(stored)}::text::jsonb, now(), now())\`;
				await tx\`insert into room_type_media (tenant_id, room_type_id, asset_id, position)
					values (\${TENANT}, \${ROOM_TYPE}, \${asset}, \${position})\`;
			}
		});
		await sql.close();
		process.stdout.write(ids[0]);
	`,
		{
			TENANT: tenant,
			PROPERTY: property,
			ROOM_TYPE: roomType,
			DIR: dir,
			PHOTOS: String(PHOTOS),
			RENDITIONS: JSON.stringify(renditions)
		}
	).trim();
}

/**
 * The few calls of an `APIRequestContext` the e2e seed helpers make, over `fetch` with the session cookie kept:
 * Playwright's own request client does not run under Bun (it cannot parse a relative response URL).
 */
function apiClient(base: string): APIRequestContext {
	let cookie = '';
	const call =
		(method: string) =>
		async (path: string, options: { headers?: Record<string, string>; data?: object } = {}) => {
			const response = await fetch(new URL(path, base), {
				method,
				headers: {
					...options.headers,
					...(cookie ? { cookie } : {}),
					...(options.data ? { 'content-type': 'application/json' } : {})
				},
				body: options.data ? JSON.stringify(options.data) : undefined
			});
			const set = response.headers.get('set-cookie');
			if (set) cookie = set.split(';')[0];
			const body = await response.text();
			return {
				status: () => response.status,
				text: async () => body,
				json: async () => JSON.parse(body)
			};
		};
	return { get: call('GET'), post: call('POST'), put: call('PUT') } as unknown as APIRequestContext;
}

/** Each page's LCP in every run, from the reports `@lhci/cli` wrote to `.lighthouseci`. */
function lcps(): Map<string, number[]> {
	const path = join(SITE, '.lighthouseci/manifest.json');
	if (!existsSync(path)) return new Map();
	const manifest: { url: string; jsonPath: string }[] = JSON.parse(readFileSync(path, 'utf8'));
	const byPage = new Map<string, number[]>();
	for (const entry of manifest) {
		const report = JSON.parse(readFileSync(entry.jsonPath, 'utf8'));
		const lcp: number = report.audits['largest-contentful-paint'].numericValue;
		byPage.set(entry.url, [...(byPage.get(entry.url) ?? []), Math.round(lcp)]);
	}
	return byPage;
}

const renditions = mkdtempSync(join(tmpdir(), 'goodfolk-lhci-'));
try {
	run(
		['cargo', 'build', '-q', ...CARGO_JOBS, '--release', '-p', 'core-api', '-p', 'media-svc'],
		ROOT
	);
	start([join(ROOT, 'target/release/core-api')], ROOT, {
		PORT: String(API_PORT),
		DATABASE_URL: database,
		// The README's local development key, never used for real guest data.
		GUEST_ID_KEY: 'HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=',
		...SERVICE_AUTH,
		MEDIA_SVC_URL: `http://localhost:${MEDIA_PORT}`,
		PUSH_ENDPOINT_MEDIA_PROCESS: `http://localhost:${MEDIA_PORT}/work`,
		PUSH_ENDPOINT_INVOICE_RENDER: `http://localhost:${MEDIA_PORT}/render`,
		PUSH_ENDPOINT_REPORT_RENDER: `http://localhost:${MEDIA_PORT}/render-report`,
		// No jobs-svc: nothing here books, so no mail is sent.
		PUSH_ENDPOINT_MAIL_SEND: `http://localhost:${JOBS_PORT}/mail`,
		IBE_PUBLIC_ORIGIN: SITE_ORIGIN,
		IBE_SSR_SECRET,
		RUST_LOG: 'warn'
	});
	start([join(ROOT, 'target/release/media-svc')], ROOT, {
		PORT: String(MEDIA_PORT),
		...SERVICE_AUTH,
		PUSH_ENDPOINT_MEDIA_RESULTS: `${API}/internal/events`,
		RUST_LOG: 'warn'
	});
	run([process.execPath, 'run', 'build'], SITE);
	start(
		[
			process.execPath,
			'node_modules/vite/bin/vite.js',
			'preview',
			'--port',
			String(SITE_PORT),
			'--strictPort'
		],
		SITE,
		{
			PUBLIC_API_ORIGIN: API,
			IBE_API_ORIGIN: API,
			PUBLIC_IBE_ORIGIN: SITE_ORIGIN,
			IBE_SSR_SECRET
		}
	);
	await Promise.all([
		waitFor(`${API}/readyz`),
		waitFor(`http://localhost:${MEDIA_PORT}/healthz`),
		waitFor(SITE_ORIGIN)
	]);

	const api = apiClient(API);
	const hotel = await seedHotel(api);
	const site = await (await api.get(`/api/public/v1/ibe/${hotel.code}/site`)).json();
	const deluxe: string = site.room_types.find(
		(type: { name: string }) => type.name === 'Deluxe'
	).id;
	const hero = seedGallery(hotel.tenant, hotel.propertyId, deluxe, renditions);
	const saved = await api.put(hotel.siteSettings, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': '"1"' },
		data: { enabled: true, description: 'A quiet hotel by the lagoon.', hero_asset_id: hero }
	});
	if (saved.status() !== 200)
		throw new Error(`saving the hero: ${saved.status()} ${await saved.text()}`);

	const landing = `${SITE_ORIGIN}/${hotel.code}`;
	const stay = new URLSearchParams({
		check_in: addDays(hotel.businessDate, 1),
		check_out: addDays(hotel.businessDate, 3),
		residency: 'non_resident',
		rooms: '2-0'
	});
	const results = `${landing}/rooms?${stay}`;
	const flags = [
		'--headless=new',
		...(process.env.PLAYWRIGHT_NO_SANDBOX === '1' ? ['--no-sandbox'] : [])
	];
	rmSync(join(SITE, '.lighthouseci'), { recursive: true, force: true });
	const lhci = Bun.spawnSync(
		[
			process.execPath,
			'x',
			LHCI,
			'autorun',
			'--config=lighthouserc.json',
			`--collect.url=${landing}`,
			`--collect.url=${results}`,
			`--collect.settings.chromeFlags=${flags.join(' ')}`
		],
		// CHROME_PATH, not --collect.chromePath: lhci's health check looks for Chrome there before it collects.
		{
			cwd: SITE,
			env: { ...process.env, CHROME_PATH: chromium.executablePath() },
			stdout: 'inherit',
			stderr: 'inherit'
		}
	);
	for (const [url, runs] of lcps()) {
		console.log(`LCP ${url.replace(SITE_ORIGIN, '')}: ${runs.join(', ')} ms (gate 1500 ms)`);
	}
	process.exitCode = lhci.exitCode === 0 ? 0 : 1;
} finally {
	for (const child of started) child.kill();
	await Promise.all(started.map((child) => child.exited));
	rmSync(renditions, { recursive: true, force: true });
}
