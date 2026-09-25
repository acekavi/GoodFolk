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

export default defineConfig({
	testDir: 'tests/e2e',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	// Performance checks (@perf) run only when asked for: E2E_PERF=1.
	grepInvert: process.env.E2E_PERF === '1' ? undefined : /@perf/,
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
			env: { PORT: String(API_PORT), DATABASE_URL: database, RUST_LOG: 'warn' },
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
