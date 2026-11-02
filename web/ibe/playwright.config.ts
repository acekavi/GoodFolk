import { mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig, devices } from '@playwright/test';

// End-to-end tests drive the built booking site and the built staff app against a real API and Postgres, all on
// localhost. E2E_DATABASE_URL is a migrated database, as the API role, and E2E_OWNER_URL the same one as its schema
// owner (see web/pms/README.md "End-to-end tests").
const database = process.env.E2E_DATABASE_URL;
if (!database) throw new Error('set E2E_DATABASE_URL to a migrated database, as goodfolk_api');

// Chromium's sandbox stays on, as in CI. PLAYWRIGHT_NO_SANDBOX=1 turns it off for local machines that
// cannot start it (no unprivileged user namespaces); the suite only ever loads localhost pages.
const noSandbox = process.env.PLAYWRIGHT_NO_SANDBOX === '1';
if (noSandbox && process.env.CI) throw new Error('PLAYWRIGHT_NO_SANDBOX is for local runs only');

// Ports apart from the staff app's own suite (web/pms/playwright.config.ts), so that the two can run side by side.
// The site's is core-api's development IBE_PUBLIC_ORIGIN, which its CORS admits.
const API_PORT = 18180;
const SITE_PORT = 5174;
const PMS_PORT = 4273;
const MEDIA_PORT = 8190;
const JOBS_PORT = 8191;

// The dev mailer jobs-svc sends the confirmation email with writes each message as a file here; the tests read the
// manage link out of it. Set before the workers start, so that they inherit it.
process.env.MAIL_OUTBOX_DIR ??= join(tmpdir(), 'goodfolk-e2e-ibe-mail-outbox');
mkdirSync(process.env.MAIL_OUTBOX_DIR, { recursive: true });

// core-api, media-svc and jobs-svc push/call to each other with a test-only shared secret.
const SERVICE_AUTH = {
	SVC_AUTH: 'secret',
	INTERNAL_SHARED_SECRET: 'e2e-internal-shared-secret-0123456789abcdef',
	EVENT_BUS: 'push'
};

// Shared by core-api and the site's server, so that a server render counts against the guest, not 127.0.0.1.
const IBE_SSR_SECRET = 'e2e-ibe-ssr-secret-0123456789abcdef-0123';

// Cloudflare's published Turnstile test keys: the widget always passes, and the server check calls Cloudflare for
// real, so this suite needs the internet.
const TURNSTILE_SITEKEY = '1x00000000000000000000AA';
const TURNSTILE_SECRET = '1x0000000000000000000000000000000AA';

export default defineConfig({
	testDir: 'tests/e2e',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 1 : 0,
	reporter: process.env.CI ? [['github'], ['list']] : 'list',
	use: {
		baseURL: `http://localhost:${SITE_PORT}`,
		trace: 'retain-on-failure'
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'], launchOptions: { chromiumSandbox: !noSandbox } }
		}
	],
	webServer: [
		{
			command: 'cargo run -q -p core-api',
			cwd: '../..',
			url: `http://localhost:${API_PORT}/readyz`,
			// GUEST_ID_KEY is the README's local development key, never used for real guest data.
			env: {
				PORT: String(API_PORT),
				DATABASE_URL: database,
				GUEST_ID_KEY: 'HU5/qSn58655epIBi671lsojvXir+VQZ0MzXdTrKk6o=',
				...SERVICE_AUTH,
				MEDIA_SVC_URL: `http://localhost:${MEDIA_PORT}`,
				PUSH_ENDPOINT_MEDIA_PROCESS: `http://localhost:${MEDIA_PORT}/work`,
				PUSH_ENDPOINT_INVOICE_RENDER: `http://localhost:${MEDIA_PORT}/render`,
				PUSH_ENDPOINT_REPORT_RENDER: `http://localhost:${MEDIA_PORT}/render-report`,
				PUSH_ENDPOINT_MAIL_SEND: `http://localhost:${JOBS_PORT}/mail`,
				JOBS_SVC_URL: `http://localhost:${JOBS_PORT}`,
				IBE_PUBLIC_ORIGIN: `http://localhost:${SITE_PORT}`,
				IBE_SSR_SECRET,
				TURNSTILE_SECRET,
				RUST_LOG: 'warn'
			},
			reuseExistingServer: !process.env.CI,
			timeout: 600_000
		},
		{
			command: 'cargo run -q -p media-svc',
			cwd: '../..',
			url: `http://localhost:${MEDIA_PORT}/healthz`,
			env: {
				PORT: String(MEDIA_PORT),
				...SERVICE_AUTH,
				PUSH_ENDPOINT_MEDIA_RESULTS: `http://localhost:${API_PORT}/internal/events`,
				RUST_LOG: 'warn'
			},
			reuseExistingServer: !process.env.CI,
			timeout: 600_000
		},
		{
			// Sends the confirmation email for core-api. It never ticks itself and never fetches from the Central
			// Bank in tests: the suite seeds its own exchange rates.
			command: 'cargo run -q -p jobs-svc',
			cwd: '../..',
			url: `http://localhost:${JOBS_PORT}/healthz`,
			env: {
				PORT: String(JOBS_PORT),
				DATABASE_URL: database,
				...SERVICE_AUTH,
				JOBS_TICK: 'off',
				MAIL_OUTBOX_DIR: process.env.MAIL_OUTBOX_DIR,
				APP_BASE_URL: `http://localhost:${PMS_PORT}`,
				IBE_PUBLIC_ORIGIN: `http://localhost:${SITE_PORT}`,
				FX_IMPORT: 'off',
				RUST_LOG: 'warn'
			},
			reuseExistingServer: !process.env.CI,
			timeout: 600_000
		},
		{
			// The production server, as the image runs it; ORIGIN is what adapter-node needs over plain HTTP.
			command: 'bun run build && node build',
			// `/` on the booking domain is a plain 404, which Playwright does not take for up.
			port: SITE_PORT,
			env: {
				PORT: String(SITE_PORT),
				ORIGIN: `http://localhost:${SITE_PORT}`,
				PUBLIC_API_ORIGIN: `http://localhost:${API_PORT}`,
				IBE_API_ORIGIN: `http://localhost:${API_PORT}`,
				PUBLIC_IBE_ORIGIN: `http://localhost:${SITE_PORT}`,
				PUBLIC_TURNSTILE_SITEKEY: TURNSTILE_SITEKEY,
				IBE_SSR_SECRET
			},
			reuseExistingServer: !process.env.CI,
			timeout: 180_000
		},
		{
			// The staff app, for the front desk's view of the booking.
			command: `bun run build && bun run preview --port ${PMS_PORT} --strictPort`,
			cwd: '../pms',
			url: `http://localhost:${PMS_PORT}`,
			env: { GOODFOLK_API: `http://localhost:${API_PORT}` },
			reuseExistingServer: !process.env.CI,
			timeout: 180_000
		}
	]
});
