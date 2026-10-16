import { rest } from './api/rest';
import { toApiError } from './api/problem';
import type { components } from './api/openapi';

export type Profile = components['schemas']['Profile'];

export async function fetchMe(): Promise<Profile> {
	const { data, error, response } = await rest.GET('/api/v1/me');
	if (!response.ok || !data) throw toApiError(error, response.status);
	return data;
}

/** Who the profile is, for tagging what the housekeeper app keeps on the device. */
export function ownerOf(profile: Profile): { userId: string; tenantId: string } {
	return { userId: profile.user_id, tenantId: profile.current_tenant ?? '' };
}

/** UI hint only; the API enforces permissions. */
export function isTenantOwner(profile: Profile): boolean {
	return profile.grants.some((grant) => grant.role === 'owner' && grant.property_id == null);
}

type Role = Profile['grants'][number]['role'];

/** Roles allowed each action, mirroring `identity::Permission` on the server. */
const ACTIONS = {
	manageRooms: ['owner', 'manager'],
	blockRooms: ['owner', 'manager', 'front_desk'],
	manageRates: ['owner', 'manager'],
	viewReservations: ['owner', 'manager', 'front_desk', 'housekeeping', 'accountant'],
	manageReservations: ['owner', 'manager', 'front_desk'],
	/** Check a room in, undo a same-day check-in, and check it out; mirrors `FrontDeskCheckIn`. */
	frontDeskCheckIn: ['owner', 'manager', 'front_desk'],
	/** The housekeeping board, issues and My rooms; mirrors `HousekeepingView`. */
	housekeepingView: ['owner', 'manager', 'front_desk', 'housekeeping', 'hk_supervisor'],
	/** Any condition, bulk changes, assignment, Copy yesterday and resolving issues; mirrors `HousekeepingSupervise`. */
	housekeepingSupervise: ['owner', 'manager', 'hk_supervisor'],
	/** Report a room issue; mirrors `RoomIssuesReport`. */
	roomIssuesReport: ['owner', 'manager', 'hk_supervisor', 'housekeeping', 'front_desk'],
	/** Upload, reorder, caption and remove gallery photos; mirrors `MediaManage`. */
	mediaManage: ['owner', 'manager'],
	/** Upload, view and delete guests' ID documents; mirrors `GuestDocumentsManage`. */
	guestDocumentsManage: ['owner', 'manager', 'front_desk']
} satisfies Record<string, Role[]>;

/** UI hint only; the API enforces permissions. A grant counts tenant-wide or for `propertyId`. */
export function can(profile: Profile, action: keyof typeof ACTIONS, propertyId: string): boolean {
	const roles: Role[] = ACTIONS[action];
	return profile.grants.some(
		(grant) =>
			roles.includes(grant.role) && (grant.property_id == null || grant.property_id === propertyId)
	);
}
