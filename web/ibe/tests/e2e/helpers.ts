import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, type APIRequestContext, type Page } from '@playwright/test';

/** The built staff app, which proxies `/api` and `/graphql` to the API (see playwright.config.ts). */
/** The booking site's port: core-api's development IBE_PUBLIC_ORIGIN, which its CORS admits. */
export const SITE_PORT = 5281;
export const SITE_ORIGIN = `http://localhost:${SITE_PORT}`;
export const PMS = 'http://localhost:4273';
export const PASSWORD = 'a long enough password';

/** Runs `script` with Bun as the schema owner (`E2E_OWNER_URL`, as `process.env.URL`) and returns what it prints. */
export function runAsOwner(script: string, env: Record<string, string> = {}): string {
	const owner = process.env.E2E_OWNER_URL;
	if (!owner) throw new Error('set E2E_OWNER_URL to the e2e database, as goodfolk_owner');
	return execFileSync('bun', ['-e', script], {
		env: { ...process.env, ...env, URL: owner },
		stdio: 'pipe',
		encoding: 'utf8'
	});
}

/** `YYYY-MM-DD` plus `days`. */
export function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

async function post(api: APIRequestContext, path: string, data: object) {
	const response = await api.post(path, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data
	});
	expect(response.status(), await response.text()).toBe(201);
	return response.json();
}

/** A property a test made, with its owner signed in on `api`. */
export interface Hotel {
	ownerEmail: string;
	tenant: string;
	propertyId: string;
	/** The code the site is addressed by: `/<code>`. */
	code: string;
	name: string;
	businessDate: string;
	/** The REST path of the property's booking site settings. */
	siteSettings: string;
}

/**
 * Signs up an owner on `api` (a context on the staff app) and makes a property with two room types, Deluxe and
 * Standard, four rooms each, and one IBE plan, Best available, at USD 100 a night for two adults on either
 * type for the next 5 nights. Its booking site is on, unless `enabled` is false. Rates are per property, so it
 * gets its own, as every foreign-currency posting needs one.
 */
export async function seedHotel(api: APIRequestContext, enabled = true): Promise<Hotel> {
	const ownerEmail = `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	const signedUp = await api.post('/api/v1/auth/signup', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			display_name: 'Nimal Perera',
			email: ownerEmail,
			password: PASSWORD,
			tenant_name: 'Lagoon Hotels'
		}
	});
	expect(signedUp.status(), await signedUp.text()).toBe(201);
	// As following the link of the sign-up's verification email would: turning a booking site on needs it.
	runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		await sql\`update app_user set email_verified_at = now() where email = \${process.env.EMAIL}\`;
		await sql.close();
	`,
		{ EMAIL: ownerEmail }
	);
	const me = await api.get('/api/v1/me');
	const tenant: string = (await me.json()).current_tenant;

	const code = `B${Math.random().toString(36).slice(2, 8).toUpperCase()}`;
	const name = `Hotel ${code}`;
	const property = await post(api, '/api/v1/properties', {
		code,
		name,
		base_currency: 'LKR',
		timezone: 'Asia/Colombo'
	});
	const propertyId: string = property.id;
	const path = `/api/v1/properties/${propertyId}`;
	const graphql = await api.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { query: '{ properties { id businessDate } }' }
	});
	const businessDate: string = (await graphql.json()).data.properties.find(
		(listed: { id: string }) => listed.id === propertyId
	).businessDate;

	runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			await tx\`insert into fx_rate (tenant_id, property_id, date, currency, rate_to_base, source, source_date)
				values (\${process.env.TENANT}, \${process.env.PROPERTY}, current_date - 365, 'USD', 300::numeric,
					'manual', current_date - 365)\`;
		});
		await sql.close();
	`,
		{ TENANT: tenant, PROPERTY: propertyId }
	);

	const types: { id: string }[] = [];
	for (const [typeCode, typeName, first] of [
		['DLX', 'Deluxe', 101],
		['STD', 'Standard', 201]
	] as const) {
		const type = await post(api, `${path}/room-types`, {
			code: typeCode,
			name: typeName,
			base_occupancy: 2,
			max_adults: 2,
			max_children: 1,
			max_occupancy: 3
		});
		await post(api, `${path}/rooms/bulk`, { room_type_id: type.id, first, last: first + 3 });
		types.push(type);
	}
	const plan = await post(api, `${path}/rate-plans`, {
		code: 'BAR',
		name: 'Best available',
		kind: 'standard',
		segment: 'IBE',
		currency: 'USD',
		room_type_ids: types.map((type) => type.id)
	});
	const prices = types.flatMap((type) =>
		Array.from({ length: 5 }, (_, night) => ({
			room_type_id: type.id,
			date: addDays(businessDate, night),
			occupancy: 2,
			amount: 10_000
		}))
	);
	const priced = await api.put(`${path}/rate-plans/${plan.id}/prices`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { prices }
	});
	expect(priced.status(), await priced.text()).toBe(204);

	const hotel: Hotel = {
		ownerEmail,
		tenant,
		propertyId,
		code,
		name,
		businessDate,
		siteSettings: `${path}/ibe-site`
	};
	if (enabled) await setSite(api, hotel, true, 0);
	return hotel;
}

/** Saves the site's settings at `version` (0 for the first save), on or off, and returns the version after. */
export async function setSite(
	api: APIRequestContext,
	hotel: Hotel,
	enabled: boolean,
	version: number
): Promise<number> {
	const saved = await api.put(hotel.siteSettings, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${version}"` },
		data: { enabled, description: 'A quiet hotel by the lagoon.' }
	});
	expect(saved.status(), await saved.text()).toBe(200);
	return version + 1;
}

/**
 * Adds a front desk user to the owner's tenant through `core-api add-staff`, as the schema owner, and returns their
 * email. They sign in with `PASSWORD`.
 */
export function addFrontDesk(ownerEmail: string): string {
	const owner = process.env.E2E_OWNER_URL;
	if (!owner) throw new Error('set E2E_OWNER_URL to the e2e database, as goodfolk_owner');
	const email = `staff-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	// The password is read from a file, never passed as an argument.
	const dir = mkdtempSync(join(tmpdir(), 'goodfolk-staff-'));
	try {
		const passwordFile = join(dir, 'password');
		writeFileSync(passwordFile, PASSWORD, { mode: 0o600 });
		execFileSync(
			'cargo',
			[
				'run',
				'-q',
				'-p',
				'core-api',
				'--',
				'add-staff',
				'--owner-email',
				ownerEmail,
				'--email',
				email,
				'--password-file',
				passwordFile,
				'--display-name',
				'Ruwan Fernando',
				'--role',
				'front_desk'
			],
			{ cwd: '../..', env: { ...process.env, DATABASE_OWNER_URL: owner }, stdio: 'pipe' }
		);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
	return email;
}

/** Signs `email` in on the staff app's login page (`page` is on its origin) and waits for the property list. */
export async function signIn(page: Page, email: string): Promise<void> {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await page.waitForURL((url) => url.pathname === '/');
	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
}

/** How many reservations the property has, whatever their source or status, counted as the schema owner. */
export function reservationCount(hotel: Hotel): number {
	const out = runAsOwner(
		`
		const sql = new Bun.SQL(process.env.URL);
		const rows = await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			return tx\`select count(*)::int as n from reservation where property_id = \${process.env.PROPERTY}\`;
		});
		process.stdout.write(String(rows[0].n));
		await sql.close();
	`,
		{ TENANT: hotel.tenant, PROPERTY: hotel.propertyId }
	);
	return Number(out.trim());
}

/**
 * The newest message the dev mailer wrote to `email` whose subject starts with `subject`, once jobs-svc has sent it.
 * The mailer renames each file into place whole, and its names sort in the order they were sent.
 */
export async function mailTo(
	email: string,
	subject: string
): Promise<{ subject: string; text: string }> {
	const dir = process.env.MAIL_OUTBOX_DIR;
	if (!dir) throw new Error('MAIL_OUTBOX_DIR is set by playwright.config.ts');
	let found: { subject: string; text: string } | undefined;
	await expect
		.poll(
			() => {
				const files = readdirSync(dir)
					.filter((name) => name.endsWith('.json'))
					.sort()
					.reverse();
				for (const name of files) {
					const mail: { to: string; subject: string; text: string } = JSON.parse(
						readFileSync(join(dir, name), 'utf8')
					);
					if (mail.to === email && mail.subject.startsWith(subject)) {
						found = mail;
						return true;
					}
				}
				return false;
			},
			{ message: `an email to ${email} headed "${subject}"`, timeout: 30_000 }
		)
		.toBe(true);
	return found!;
}
