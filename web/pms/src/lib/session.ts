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
	/** A property's settings, such as its check-in, check-out and audit times, and its ops alerts; mirrors `PropertiesManage`. */
	propertiesManage: ['owner', 'manager'],
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
	guestDocumentsManage: ['owner', 'manager', 'front_desk'],
	/** Folios, lines, payments and invoices; mirrors `FolioView`. */
	folioView: ['owner', 'manager', 'front_desk', 'accountant'],
	/** Charges and payments; mirrors `FolioPost`. */
	folioPost: ['owner', 'manager', 'front_desk'],
	/** Reversals, transfers, refunds, routing, credit notes and tonight's charges; mirrors `FolioAdjust`. */
	folioAdjust: ['owner', 'manager'],
	/** Issue invoices and open their PDFs; mirrors `InvoiceIssue`. */
	invoiceIssue: ['owner', 'manager', 'front_desk', 'accountant'],
	/** Tax rules, outlets and the property's tax settings; mirrors `TaxSettingsManage`. */
	taxSettingsManage: ['owner'],
	/** Manual exchange rates; mirrors `FxOverride`. */
	fxOverride: ['owner', 'manager'],
	/** POS bills; mirrors `PosPost`. */
	posPost: ['owner', 'manager'],
	/** The night-audit review, starting and retrying the close, and the audit history; mirrors `NightAudit`. */
	nightAudit: ['owner', 'manager', 'front_desk'],
	/** Linen items, locations, par, stock, the movement ledger and laundry batches; mirrors `LinenView`. */
	linenView: ['owner', 'manager', 'hk_supervisor', 'housekeeping'],
	/** Receive, transfer, send and return linen (laundry batches out and back); mirrors `LinenMove`. */
	linenMove: ['owner', 'manager', 'hk_supervisor', 'housekeeping'],
	/** Linen items, locations, par levels and write-offs; mirrors `LinenManage`. */
	linenManage: ['owner', 'manager', 'hk_supervisor'],
	/** Guests' laundry orders; mirrors `LaundryOrders`. */
	laundryOrders: ['owner', 'manager', 'front_desk', 'hk_supervisor', 'housekeeping'],
	/** The guest laundry price list; mirrors `LaundryPricesManage`. */
	laundryPricesManage: ['owner', 'manager']
} satisfies Record<string, Role[]>;

/** UI hint only; the API enforces permissions. A grant counts tenant-wide or for `propertyId`. */
export function can(profile: Profile, action: keyof typeof ACTIONS, propertyId: string): boolean {
	const roles: Role[] = ACTIONS[action];
	return profile.grants.some(
		(grant) =>
			roles.includes(grant.role) && (grant.property_id == null || grant.property_id === propertyId)
	);
}
