import { graphql } from './api/gql';
import type { InvitationsQuery, MembersQuery, PermissionMatrixQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import type { components } from './api/openapi';
import type { Profile } from './session';

export type Role = components['schemas']['Role'];
/** A grant as REST gives and takes it: a role, and a property or none (every property). */
export type Grant = components['schemas']['GrantSpec'];

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

/** The Staff page: the tenant's members and its invitations still waiting (pending or lapsed). */
export const MembersDocument = graphql(`
	query Members {
		members {
			userId
			email
			displayName
			grants {
				role
				propertyId
			}
			joinedAt
			version
		}
	}
`);

export const InvitationsDocument = graphql(`
	query Invitations {
		invitations {
			id
			email
			displayName
			grants {
				role
				propertyId
			}
			status
			createdAt
			expiresAt
		}
	}
`);

/** The Permissions page: every permission, what it allows, and the roles that hold it. */
export const PermissionMatrixDocument = graphql(`
	query PermissionMatrix {
		permissionMatrix {
			permission
			allows
			roles
		}
	}
`);

export type MemberRow = MembersQuery['members'][number];
export type InvitationRow = InvitationsQuery['invitations'][number];
export type PermissionRow = PermissionMatrixQuery['permissionMatrix'][number];

/** Under the server's `staff:<tenant>` event, which every invitation and membership change queues. */
export function staffKey(tenantId: string) {
	return [`staff:${tenantId}`] as const;
}

export function membersKey(tenantId: string) {
	return [...staffKey(tenantId), 'members'] as const;
}

export function invitationsKey(tenantId: string) {
	return [...staffKey(tenantId), 'invitations'] as const;
}

/** Not named by any event: the matrix is fixed by the server's role table. */
export const permissionMatrixKey = ['permission-matrix'] as const;

export async function fetchMembers(signal?: AbortSignal) {
	return (await query(MembersDocument, undefined, signal)).members;
}

/** The invitations still to act on: pending, or lapsed (which can be resent). */
export async function fetchOpenInvitations(signal?: AbortSignal) {
	return (await query(InvitationsDocument, undefined, signal)).invitations.filter(
		(invitation) => invitation.status === 'PENDING' || invitation.status === 'EXPIRED'
	);
}

export async function fetchPermissionMatrix(signal?: AbortSignal) {
	return (await query(PermissionMatrixDocument, undefined, signal)).permissionMatrix;
}

/** Every role, in the order the UI lists them. */
export const ROLES: readonly Role[] = [
	'owner',
	'manager',
	'front_desk',
	'housekeeping',
	'hk_supervisor',
	'accountant',
	'pos'
];

/** The roles a manager may give and act on, mirroring the server's `JUNIOR_ROLES`. */
export const JUNIOR_ROLES: readonly Role[] = [
	'front_desk',
	'housekeeping',
	'hk_supervisor',
	'accountant',
	'pos'
];

/** A GraphQL grant (`{role: 'FRONT_DESK', propertyId}`) as REST names it. */
export function grantFromNode(grant: { role: string; propertyId?: string | null }): Grant {
	return { role: grant.role.toLowerCase() as Role, property_id: grant.propertyId ?? null };
}

/** A grant as a line, with the property's name from `names`; a property not listed reads as another one. */
export function grantText(grant: Grant, names: ReadonlyMap<string, string>): string {
	const id = grant.property_id;
	return grantLabel({
		role: grant.role,
		property_name: id == null ? null : (names.get(id) ?? 'Another property')
	});
}

function isOwner(actor: readonly Grant[]): boolean {
	return actor.some((grant) => grant.role === 'owner' && grant.property_id == null);
}

function tenantWideManager(actor: readonly Grant[]): boolean {
	return actor.some((grant) => grant.role === 'manager' && grant.property_id == null);
}

function manages(actor: readonly Grant[], propertyId: string): boolean {
	return actor.some((grant) => grant.role === 'manager' && grant.property_id === propertyId);
}

/**
 * Whether `actor` may give `grant`, mirroring the server's `check_may_give` (which enforces it regardless): nobody
 * gives `owner`; owners give anything else; a manager gives a junior role at a property they manage, or anywhere
 * (and tenant-wide) with a tenant-wide `manager` grant.
 */
export function mayGive(actor: readonly Grant[], grant: Grant): boolean {
	if (grant.role === 'owner') return false;
	if (isOwner(actor)) return true;
	if (!JUNIOR_ROLES.includes(grant.role)) return false;
	if (tenantWideManager(actor)) return true;
	return grant.property_id != null && manages(actor, grant.property_id);
}

/**
 * Whether `actor` may change or remove a member (or resend or revoke an invitation) holding `held`, mirroring the
 * server's `check_may_act_on`: owners act on anyone; a manager only when every grant held is one they could give.
 */
export function mayActOn(actor: readonly Grant[], held: readonly Grant[]): boolean {
	if (isOwner(actor)) return true;
	if (!actor.some((grant) => grant.role === 'manager')) return false;
	return held.every((grant) => mayGive(actor, grant));
}

/** Whether the Change roles and Remove actions are shown for `member`: never for oneself. */
export function mayEditMember(
	profile: Pick<Profile, 'user_id' | 'grants'>,
	member: { userId: string; grants: readonly Grant[] }
): boolean {
	return member.userId !== profile.user_id && mayActOn(profile.grants, member.grants);
}

/** A grant row's property choice: `ALL` for every property (tenant-wide), otherwise a property id. */
export const ALL = 'all';

/**
 * The roles a grant row may pick: what `actor` may give, plus the roles of `held` (a member's grants, kept as they
 * are even when the actor could not give them, as an owner's own `owner`).
 */
export function roleOptions(actor: readonly Grant[], held: readonly Grant[] = []): Role[] {
	const manager = actor.some((grant) => grant.role === 'manager');
	const givable = (role: Role) =>
		role !== 'owner' && (isOwner(actor) || (manager && JUNIOR_ROLES.includes(role)));
	const kept = new Set(held.map((grant) => grant.role));
	return ROLES.filter((role) => kept.has(role) || givable(role));
}

/**
 * Where a grant row of `role` may apply, as `ALL` or property ids in the order of `propertyIds`: where `actor` may
 * give it, plus where `held` already has it. `owner` is always tenant-wide.
 */
export function scopeOptions(
	actor: readonly Grant[],
	role: Role,
	propertyIds: readonly string[],
	held: readonly Grant[] = []
): string[] {
	const kept = held.filter((grant) => grant.role === role).map((grant) => grant.property_id ?? ALL);
	const extra = kept.filter((scope) => scope !== ALL && !propertyIds.includes(scope));
	return [ALL, ...propertyIds, ...extra].filter(
		(scope) =>
			(role !== 'owner' || scope === ALL) &&
			(kept.includes(scope) || mayGive(actor, { role, property_id: scope === ALL ? null : scope }))
	);
}

export interface GrantRowDraft {
	role: Role;
	/** `ALL` or a property id. */
	scope: string;
}

/** The rows as the grants REST takes, each once. */
export function grantsBody(rows: readonly GrantRowDraft[]): Grant[] {
	const seen = new Set<string>();
	const grants: Grant[] = [];
	for (const row of rows) {
		const key = `${row.role}/${row.scope}`;
		if (seen.has(key)) continue;
		seen.add(key);
		grants.push({ role: row.role, property_id: row.scope === ALL ? null : row.scope });
	}
	return grants;
}

/** A member's grants as rows of the Change roles dialog. */
export function grantRows(grants: readonly Grant[]): GrantRowDraft[] {
	return grants.map((grant) => ({ role: grant.role, scope: grant.property_id ?? ALL }));
}

/** A new row: the first role `actor` may give (front desk when they may), where they may give it. */
export function newGrantRow(
	actor: readonly Grant[],
	propertyIds: readonly string[]
): GrantRowDraft | null {
	const roles = roleOptions(actor);
	const role = roles.includes('front_desk') ? 'front_desk' : roles[0];
	if (!role) return null;
	const scope = scopeOptions(actor, role, propertyIds)[0];
	return scope === undefined ? null : { role, scope };
}

/** One permission of the matrix, with whether each of `ROLES` holds it. */
export interface MatrixRow {
	permission: string;
	allows: string;
	holds: boolean[];
}

/** The matrix as a table: a row per permission, in the server's order, a column per role of `ROLES`. */
export function matrixRows(rows: readonly PermissionRow[]): MatrixRow[] {
	return rows.map((row) => {
		const roles = new Set(row.roles.map((role) => role.toLowerCase()));
		return {
			permission: row.permission,
			allows: row.allows,
			holds: ROLES.map((role) => roles.has(role))
		};
	});
}
