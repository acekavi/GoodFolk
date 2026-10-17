import { defineConfig, devices } from '@playwright/test';

// End-to-end tests drive the built SPA against a real API and Postgres, all on localhost.
// E2E_DATABASE_URL is a migrated database, as the API role (see README "End-to-end tests").
const database = process.env.E2E_DATABASE_URL;
if (!database) throw new Error('set E2E_DATABASE_URL to a migrated database, as goodfolk_api');

// Chromium's sandbox stays on, as in CI. PLAYWRIGHT_NO_SANDBOX=1 turns it off for local machines that
// cannot start it (no unprivileged user namespaces); the suite only ever loads localhost pages.
const noSandbox = process.env.PLAYWRIGHT_NO_SANDBOX === '1';
if (noSandbox && process.env.CI) throw new Error('PLAYWRIGHT_NO_SANDBOX is for local runs only');

const API_PORT = 18080;
const WEB_PORT = 4173;
const MEDIA_PORT = 8090;

// core-api and media-svc push to each other with a test-only shared secret; both reach RustFS on :9000
// with its local defaults (see global-setup.ts).
const SERVICE_AUTH = {
	SVC_AUTH: 'secret',
	INTERNAL_SHARED_SECRET: 'e2e-internal-shared-secret-0123456789abcdef',
	EVENT_BUS: 'push'
};

export default defineConfig({
	testDir: 'tests/e2e',
	globalSetup: './tests/e2e/global-setup.ts',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	// Performance checks (@perf) run only when asked for: E2E_PERF=1.
	grepInvert: process.env.E2E_PERF === '1' ? undefined : /@perf/,
	// One file at a time when timing: another file's seeding beside a timed load skews it (the gallery gate
	// read 480 ms at p90 beside the tape chart's seed, 300 ms alone).
	workers: process.env.E2E_PERF === '1' ? 1 : undefined,
	retries: process.env.CI ? 1 : 0,
	reporter: process.env.CI ? [['github'], ['list']] : 'list',
	use: {
		baseURL: `http://localhost:${WEB_PORT}`,
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
				RUST_LOG: 'warn'
			},
			reuseExistingServer: !process.env.CI,
			timeout: 600_000
		},
		{
			// Processes uploads for real: renditions to RustFS, then its result back to core-api.
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
			command: `bun run build && bun run preview --port ${WEB_PORT} --strictPort`,
			url: `http://localhost:${WEB_PORT}`,
			env: { GOODFOLK_API: `http://localhost:${API_PORT}` },
			reuseExistingServer: !process.env.CI,
			timeout: 180_000
		}
	]
});
