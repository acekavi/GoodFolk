import { expect, test } from '@playwright/test';
import {
	addStaff,
	createProperty,
	inviteLink,
	PASSWORD,
	signIn,
	signUp,
	verifyEmail
} from './helpers';

// Staff end to end, in a real browser against the real API and jobs-svc, which sends the emails to the dev mailer's
// outbox (playwright.config.ts): a new owner confirms their email before inviting, the invitee accepts from the
// emailed link, removing them signs them out, and a manager's limits show in the UI and hold on the server.

test('an owner confirms their email, invites a front-desk clerk, who accepts from the emailed link; removing them signs their open page out', async ({
	page,
	browser
}) => {
	const { email } = await signUp(page);
	await createProperty(page, 'STF');
	const propertyId = page.url().split('/p/')[1];

	// Until the owner confirms their email, the Staff page asks them to, in place of Invite.
	await page.goto('/settings/staff');
	await expect(page.getByRole('heading', { name: 'Staff', exact: true })).toBeVisible();
	const verify = page.getByRole('note', { name: 'Verify your email' });
	await expect(verify).toContainText(email);
	await expect(page.getByRole('button', { name: 'Invite', exact: true })).toHaveCount(0);
	await verifyEmail(page, email);
	await page.goto('/settings/staff');
	await expect(verify).toBeHidden();
	const clerk = `clerk-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	await page.getByRole('button', { name: 'Invite', exact: true }).click();
	const invite = page.getByRole('dialog', { name: 'Invite someone' });
	await invite.getByLabel('Email').fill(clerk);
	await invite.getByLabel('Name').fill('Kasun Fernando');
	await expect(invite.getByLabel('Role 1')).toHaveValue('front_desk');
	await invite.getByLabel('Where 1').selectOption({ label: 'STF · Hotel STF' });
	await invite.getByRole('button', { name: 'Send invitation' }).click();
	await expect(invite).toBeHidden();
	const invitations = page.getByRole('table', { name: 'Invitations' });
	await expect(invitations.getByRole('row').filter({ hasText: clerk })).toContainText(
		'Front desk · Hotel STF'
	);

	// The clerk follows the link from the email and sets a password.
	const context = await browser.newContext();
	const clerkPage = await context.newPage();
	await clerkPage.goto(await inviteLink(clerk));
	await expect(clerkPage.getByRole('heading', { name: 'Join Lagoon Hotels' })).toBeVisible();
	await expect(clerkPage.getByText('Front desk · Hotel STF')).toBeVisible();
	await clerkPage.getByLabel(/Set a password/).fill(PASSWORD);
	await clerkPage.getByRole('button', { name: 'Accept' }).click();
	await expect(clerkPage).toHaveURL(new RegExp(`/p/${propertyId}$`));
	const nav = clerkPage.getByRole('navigation', { name: 'Property' });
	await expect(nav.getByRole('link', { name: 'Reservations' })).toBeVisible();
	await expect(nav.getByRole('link', { name: 'Night audit' })).toBeVisible();
	await clerkPage.goto('/settings/staff');
	await expect(clerkPage.getByText('Only owners and managers manage staff.')).toBeVisible();

	// The link is single use.
	const again = await context.newPage();
	await again.goto(await inviteLink(clerk));
	await expect(again.getByRole('alert')).toContainText('This invitation is no longer valid');

	// The owner's list moves the clerk from the invitations to the members.
	await expect(page.getByRole('table', { name: 'Members' })).toContainText(clerk);
	await expect(invitations).toBeHidden();

	// Removing the clerk ends their sessions. The idle page signs itself out: the server ends its event stream, the
	// browser's reconnect is refused with a 401, and the page's profile check then sends it to the sign-in page.
	page.once('dialog', (dialog) => dialog.accept());
	await page.getByRole('button', { name: 'Remove Kasun Fernando' }).click();
	await expect(page.getByRole('status')).toContainText('Kasun Fernando has been removed.');
	await expect(clerkPage).toHaveURL(/\/login/);
	await context.close();
});

test('a manager invites housekeeping, but cannot give the manager role, in the UI or to the server', async ({
	page
}) => {
	const { email } = await signUp(page);
	await createProperty(page, 'MGR');
	const manager = addStaff(email, 'manager', 'Mala Jayasuriya');
	await page.context().clearCookies();
	await signIn(page, manager.email);

	await page.goto('/settings/staff');
	await page.getByRole('button', { name: 'Invite', exact: true }).click();
	const invite = page.getByRole('dialog', { name: 'Invite someone' });
	const roles = invite.getByLabel('Role 1');
	await expect(roles.getByRole('option', { name: 'Housekeeping', exact: true })).toBeAttached();
	await expect(roles.getByRole('option', { name: 'Manager', exact: true })).toHaveCount(0);
	await expect(roles.getByRole('option', { name: 'Owner', exact: true })).toHaveCount(0);

	const hk = `hk-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	await invite.getByLabel('Email').fill(hk);
	await invite.getByLabel('Name').fill('Nuwan Bandara');
	await roles.selectOption('housekeeping');
	await invite.getByRole('button', { name: 'Send invitation' }).click();
	await expect(invite).toBeHidden();
	await expect(page.getByRole('table', { name: 'Invitations' })).toContainText(hk);

	// A forced request for the manager role is refused.
	const forced = await page.request.post('/api/v1/invitations', {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data: {
			email: `forced-${Date.now()}@example.com`,
			display_name: 'Forced Manager',
			grants: [{ role: 'manager', property_id: null }]
		}
	});
	expect(forced.status(), await forced.text()).toBe(403);
});

test('a till signs in to no pages, only a sign-out', async ({ page, browser }) => {
	const { email: owner } = await signUp(page);
	await createProperty(page, 'TIL');
	const { email } = addStaff(owner, 'pos', 'Till One');

	const context = await browser.newContext();
	const till = await context.newPage();
	await till.goto('/login');
	await till.getByLabel('Email').fill(email);
	await till.getByLabel('Password').fill(PASSWORD);
	await till.getByRole('button', { name: 'Sign in' }).click();
	const panel = till.getByText('This account is for a point-of-sale till and has no pages');
	await expect(panel).toBeVisible();
	await expect(till.getByRole('navigation')).toHaveCount(0);
	await expect(till.getByRole('heading', { name: 'Properties' })).toHaveCount(0);
	await till.goto('/settings');
	await expect(panel).toBeVisible();
	await expect(till.getByRole('link', { name: 'Staff and settings' })).toHaveCount(0);

	await till.getByRole('button', { name: 'Sign out' }).click();
	await expect(till).toHaveURL(/\/login/);
	await context.close();
});
