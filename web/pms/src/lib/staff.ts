import type { components } from './api/openapi';
import type { Profile } from './session';

type Role = components['schemas']['Role'];

const ROLE_LABELS: Record<Role, string> = {
	owner: 'Owner',
	manager: 'Manager',
	front_desk: 'Front desk',
	housekeeping: 'Housekeeping',
	hk_supervisor: 'Housekeeping supervisor',
	accountant: 'Accountant',
	pos: 'POS'
};

/** How a role reads in the UI. */
export function roleLabel(role: Role): string {
	return ROLE_LABELS[role];
}

/** One invitation grant as a line: the role and where it applies. */
export function grantLabel(grant: components['schemas']['PreviewGrant']): string {
	return `${roleLabel(grant.role)} · ${grant.property_name ?? 'All properties'}`;
}

/** The sign-in page's address with a return to the invitation page, so the person lands back to accept. */
export function inviteSignInHref(token: string): string {
	return `/login?redirect=${encodeURIComponent(`/invite/${encodeURIComponent(token)}`)}`;
}

/** Whether the signed-in account is the one the invitation is for (emails compare case-insensitively). */
export function isInvitee(profile: Pick<Profile, 'email'>, invitedEmail: string): boolean {
	return profile.email.toLowerCase() === invitedEmail.toLowerCase();
}

/**
 * The property to open after accepting: the first property the profile holds a grant for, unless a
 * tenant-wide grant already shows all of them (then the property list is the better start).
 */
export function landingProperty(profile: Pick<Profile, 'grants'>): string | null {
	if (profile.grants.some((grant) => grant.property_id == null)) return null;
	return profile.grants.find((grant) => grant.property_id != null)?.property_id ?? null;
}
