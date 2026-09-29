import { graphql } from './api/gql';
import type { AccountsQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { formatMoney } from './rates';

/** The Accounts page's list: companies and travel agents a reservation can be billed to. */
export const AccountsDocument = graphql(`
	query Accounts($propertyId: UUID!, $search: String, $includeInactive: Boolean = false) {
		accounts(propertyId: $propertyId, search: $search, includeInactive: $includeInactive) {
			id
			kind
			name
			contact {
				email
				phone
				address
				contactName
			}
			creditLimit
			currency
			active
			version
		}
	}
`);

export type Account = AccountsQuery['accounts'][number];

/**
 * Query key shared with the server's `accounts:<property>` event (see `reservations::accounts_key` on the
 * server, and `events.ts`'s prefix-match invalidation). `accountsKey(propertyId)` alone is the prefix of
 * every search of the property, whatever `search`/`includeInactive` are, so one command invalidates all of
 * them.
 *
 * Accounts are tenant-wide (like guests), not owned by any one property, but this key is scoped by the
 * property the screen is on anyway, matching every other per-property key in this codebase
 * (`reservations_key`, `rates.ts`'s `ratePlansKey`, ...) rather than the tenant. The server's event is
 * scoped the same way, through the property the request that changed the account came in on.
 */
export function accountsKey(propertyId: string, search?: string, includeInactive?: boolean) {
	return [`accounts:${propertyId}`, search ?? '', includeInactive ?? false] as const;
}

/** Accounts whose name contains `search` (case-insensitive); inactive ones only when `includeInactive`. */
export async function fetchAccounts(
	propertyId: string,
	search?: string,
	includeInactive?: boolean,
	signal?: AbortSignal
) {
	return (await query(AccountsDocument, { propertyId, search, includeInactive }, signal)).accounts;
}

const ACCOUNT_KIND_LABELS: Record<Account['kind'], string> = {
	COMPANY: 'Company',
	TRAVEL_AGENT: 'Travel agent'
};

export function accountKindLabel(kind: Account['kind']): string {
	return ACCOUNT_KIND_LABELS[kind];
}

/** `formatMoney`, or "No limit" when the account has none set. */
export function formatCreditLimit(
	creditLimit: number | null | undefined,
	currency: string
): string {
	return creditLimit == null ? 'No limit' : formatMoney(creditLimit, currency);
}

/** How to reach an account, joined for a compact list cell; empty when nothing is on file. */
export function contactSummary(contact: Account['contact']): string {
	return [contact.contactName, contact.email, contact.phone].filter(Boolean).join(' · ');
}
