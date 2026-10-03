import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, type APIRequestContext, type Page, type Request } from '@playwright/test';

export const PASSWORD = 'a long enough password';

const persisted: Record<string, string> = JSON.parse(
	readFileSync('src/lib/api/gql/persisted-documents.json', 'utf8')
);

/** Whether `request` runs the GraphQL query `name`. The built app sends a document id, not the query text. */
export function runsQuery(request: Request, name: string): boolean {
	const body: { documentId?: string } = JSON.parse(request.postData() ?? '{}');
	return /^query (\w+)/.exec(persisted[body.documentId ?? ''] ?? '')?.[1] === name;
}

/** Signs up a new owner with a unique email and waits for the property list. */
export async function signUp(page: Page): Promise<{ email: string }> {
	const email = `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	await page.goto('/signup');
	await page.getByLabel('Your name').fill('Nimal Perera');
	await page.getByLabel('Hotel or group name').fill('Lagoon Hotels');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Create account' }).click();
	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
	return { email };
}

/** Creates a property and waits for its page. */
export async function createProperty(page: Page, code: string): Promise<void> {
	await page.getByRole('link', { name: 'Add a property' }).click();
	await page.getByLabel('Code').fill(code);
	await page.getByLabel('Name').fill(`Hotel ${code}`);
	await page.getByRole('button', { name: 'Create property' }).click();
	await expect(page.getByRole('heading', { name: `Hotel ${code}` })).toBeVisible();
}

/** Adds a room type on the Room types page, with an overbooking allowance of 0 unless given. */
export async function addRoomType(
	page: Page,
	code: string,
	name: string,
	overbooking = 0
): Promise<void> {
	const form = page.getByRole('form', { name: 'New room type' });
	await form.getByLabel('Code').fill(code);
	await form.getByLabel('Name').fill(name);
	if (overbooking) await form.getByLabel('Overbooking allowance').fill(String(overbooking));
	await form.getByRole('button', { name: 'Add room type' }).click();
	await expect(page.getByRole('cell', { name: code, exact: true })).toBeVisible();
}

/** Adds rooms `first` to `last` of one type with the bulk dialog on the Rooms page. */
export async function addRooms(page: Page, code: string, first: number, last: number) {
	await page.getByRole('button', { name: 'Add rooms…' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add rooms' });
	await dialog.getByLabel('Room type').selectOption({ label: code });
	await dialog.getByLabel('First number').fill(String(first));
	await dialog.getByLabel('Last number').fill(String(last));
	await dialog.getByLabel('Floor').fill(String(first).slice(0, 1));
	const count = last - first + 1;
	await dialog.getByRole('button', { name: `Add ${count} rooms` }).click();
	await expect(dialog).toBeHidden();
	await expect(page.getByRole('cell', { name: String(last), exact: true })).toBeVisible();
}

/** POSTs to the REST API as the signed-in user, with a fresh idempotency key, and expects 201. */
export async function post(api: APIRequestContext, path: string, data: object) {
	const response = await api.post(path, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data
	});
	expect(response.status(), await response.text()).toBe(201);
	return response.json();
}

/**
 * Takes a freshly booked room out of the room booking auto-assigned it, for flows that assign by hand or
 * need an unassigned stay, and returns the room's version after the unassign.
 */
export async function unassign(
	api: APIRequestContext,
	hotel: Hotel,
	roomId: string,
	version: number
): Promise<number> {
	const response = await api.post(`${hotel.path}/reservation-rooms/${roomId}/unassign`, {
		headers: { 'x-goodfolk-csrf': '1', 'If-Match': `"${version}"` }
	});
	expect(response.status(), await response.text()).toBe(200);
	return (await response.json()).version;
}

/** `YYYY-MM-DD` plus `days`. */
export function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

/** What `bookableHotel` set up, for booking over the REST API. */
export interface Hotel {
	/** The property's REST path, `/api/v1/properties/<id>`. */
	path: string;
	businessDate: string;
	roomTypeId: string;
	ratePlanId: string;
	guestId: string;
}

/**
 * Makes the property the page is on bookable over the REST API: room type DLX with rooms 101 onwards, a BAR
 * plan at USD 100 a night for two adults on each of `nights` nights from the business date, and one guest.
 */
export async function bookableHotel(page: Page, rooms: number, nights: number): Promise<Hotel> {
	const api = page.request;
	const propertyId = page.url().split('/p/')[1];
	const path = `/api/v1/properties/${propertyId}`;
	const properties = await api.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { query: '{ properties { id businessDate } }' }
	});
	const { data } = await properties.json();
	const businessDate: string = data.properties.find(
		(property: { id: string }) => property.id === propertyId
	).businessDate;
	const roomType = await post(api, `${path}/room-types`, {
		code: 'DLX',
		name: 'Deluxe',
		base_occupancy: 2,
		max_adults: 2,
		max_children: 1,
		max_occupancy: 3
	});
	// The API adds at most 200 rooms at a time.
	for (let first = 101; first <= 100 + rooms; first += 200) {
		await post(api, `${path}/rooms/bulk`, {
			room_type_id: roomType.id,
			first,
			last: Math.min(first + 199, 100 + rooms)
		});
	}
	const plan = await post(api, `${path}/rate-plans`, {
		code: 'BAR',
		name: 'Best available',
		kind: 'standard',
		segment: 'IBE',
		currency: 'USD',
		room_type_ids: [roomType.id]
	});
	const prices = Array.from({ length: nights }, (_, night) => ({
		room_type_id: roomType.id,
		date: addDays(businessDate, night),
		occupancy: 2,
		amount: 10_000
	}));
	const priced = await api.put(`${path}/rate-plans/${plan.id}/prices`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: { prices }
	});
	expect(priced.status(), await priced.text()).toBe(204);
	const guest = await post(api, `${path}/guests`, {
		first_name: 'Ada',
		last_name: 'Silva',
		residency: 'non_resident'
	});
	return { path, businessDate, roomTypeId: roomType.id, ratePlanId: plan.id, guestId: guest.id };
}

/**
 * Books one reservation with a DLX room on BAR, room only, for two adults, for each of `nights` (night
 * offsets from the business date, one night each), and returns its confirmation number.
 */
export async function book(
	api: APIRequestContext,
	hotel: Hotel,
	nights: number[],
	source: 'front_desk' | 'phone' | 'email' = 'front_desk'
): Promise<string> {
	const created = await post(api, `${hotel.path}/reservations`, {
		booker_guest_id: hotel.guestId,
		source,
		rooms: nights.map((night) => ({
			room_type_id: hotel.roomTypeId,
			rate_plan_id: hotel.ratePlanId,
			meal_plan: 'RO',
			check_in: addDays(hotel.businessDate, night),
			check_out: addDays(hotel.businessDate, night + 1),
			adults: 2
		}))
	});
	return created.confirmation_no;
}

/**
 * Adds a staff user with `role` (tenant-wide) to the owner's tenant through `core-api add-staff`, as the schema
 * owner (`E2E_OWNER_URL`), and returns their email. They sign in with `PASSWORD`.
 */
export function addStaff(ownerEmail: string, role: string, displayName: string): { email: string } {
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
				displayName,
				'--role',
				role
			],
			{ cwd: '../..', env: { ...process.env, DATABASE_OWNER_URL: owner }, stdio: 'pipe' }
		);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
	return { email };
}

/** Signs `email` in on the login page and waits for the property list. */
export async function signIn(page: Page, email: string): Promise<void> {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	// The sign-in's own navigation to the property list has finished, so a `goto` that follows cannot race it.
	await page.waitForURL((url) => url.pathname === '/');
	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
}

/** An assignment of a room to a housekeeper for a business date, as the board keeps it. */
export interface Assignment {
	tenant: string;
	property: string;
	date: string;
	room: string;
	user: string;
}

/**
 * Inserts an `hk_assignment` row as the schema owner (`E2E_OWNER_URL`), in the tenant's scope as row-level
 * security requires. The API only assigns for the current business date, so a past one needs this. Bun runs
 * the insert because the e2e runner has no Postgres client.
 */
export function insertAssignment(assignment: Assignment): void {
	const owner = process.env.E2E_OWNER_URL;
	if (!owner) throw new Error('set E2E_OWNER_URL to the e2e database, as goodfolk_owner');
	const script = `
		const sql = new Bun.SQL(process.env.URL);
		const a = JSON.parse(process.env.ASSIGNMENT);
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${a.tenant}, true)\`;
			await tx\`insert into hk_assignment (tenant_id, property_id, business_date, room_id, user_id)
				values (\${a.tenant}, \${a.property}, \${a.date}, \${a.room}, \${a.user})\`;
		});
		await sql.close();
	`;
	execFileSync('bun', ['-e', script], {
		env: { ...process.env, URL: owner, ASSIGNMENT: JSON.stringify(assignment) },
		stdio: 'pipe'
	});
}

/**
 * Moves a property's business date one day on, as the night audit would, as the schema owner
 * (`E2E_OWNER_URL`), in the tenant's scope as row-level security requires. Use a property the test made.
 */
export function rollBusinessDate(tenant: string, property: string): void {
	const owner = process.env.E2E_OWNER_URL;
	if (!owner) throw new Error('set E2E_OWNER_URL to the e2e database, as goodfolk_owner');
	const script = `
		const sql = new Bun.SQL(process.env.URL);
		await sql.begin(async (tx) => {
			await tx\`select set_config('app.tenant_id', \${process.env.TENANT}, true)\`;
			await tx\`update property set business_date = business_date + 1 where id = \${process.env.PROPERTY}\`;
		});
		await sql.close();
	`;
	execFileSync('bun', ['-e', script], {
		env: { ...process.env, URL: owner, TENANT: tenant, PROPERTY: property },
		stdio: 'pipe'
	});
}

/** Creates a property of `rooms` rooms, all dirty, `assigned` of them to a housekeeper; the owner's page stays on it. */
export async function housekeepingProperty(
	page: Page,
	code: string,
	rooms: number,
	assigned: number
) {
	const { email } = await signUp(page);
	await createProperty(page, code);
	await bookableHotel(page, rooms, 2);
	const propertyId = page.url().split('/p/')[1];
	// Rooms added through the API get their condition row, so the board shows every one of them.
	const { email: housekeeper } = addStaff(email, 'housekeeping', 'Kumari Silva');
	const graphql = await page.request.post('/graphql', {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			query: `{ rooms(propertyId: "${propertyId}") { id number } housekeepers(propertyId: "${propertyId}") { userId } }`
		}
	});
	const data = (await graphql.json()).data as {
		rooms: { id: string; number: string }[];
		housekeepers: { userId: string }[];
	};
	expect(data.rooms).toHaveLength(rooms);
	const path = `/api/v1/properties/${propertyId}`;
	const dirtied = await page.request.post(`${path}/rooms/conditions`, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data: { room_ids: data.rooms.map((room) => room.id), condition: 'dirty' }
	});
	expect(dirtied.status(), await dirtied.text()).toBe(200);
	const assignment = await page.request.put(`${path}/hk-assignments`, {
		headers: { 'x-goodfolk-csrf': '1' },
		data: {
			assignments: data.rooms
				.slice(0, assigned)
				.map((room) => ({ room_id: room.id, user_id: data.housekeepers[0].userId }))
		}
	});
	expect(assignment.status(), await assignment.text()).toBe(204);
	return { propertyId, housekeeper };
}
