import type { components } from './api/openapi';
import { graphql } from './api/gql';
import type { FolioContextQuery, FoliosQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { rest, unwrap } from './api/rest';
import { exponent } from './money';
import { reservationKey } from './reservations';

export const FoliosDocument = graphql(`
	query Folios($propertyId: UUID!, $reservationId: UUID, $accountId: UUID) {
		folios(propertyId: $propertyId, reservationId: $reservationId, accountId: $accountId) {
			id
			kind
			status
			reservationRoomId
			reservationId
			accountId
			version
			linesOmitted
			truncated
			lines {
				id
				source
				category
				description
				quantity
				net
				total
				currency
				baseTotal
				businessDate
				reverses
				reason
				transferId
				postedAt
				taxes {
					code
					name
					rateBp
					amount
					presentation
				}
			}
			payments {
				id
				method
				amount
				currency
				baseAmount
				details
				evidence
				refundOf
				businessDate
				transferId
				takenAt
			}
			balances {
				currency
				amount
				baseAmount
			}
			invoices {
				id
				kind
				serial
				totalLkr
				issuedAt
				credits
				pdfReady
			}
		}
	}
`);

export const FolioContextDocument = graphql(`
	query FolioContext($propertyId: UUID!) {
		propertyTaxSettings(propertyId: $propertyId) {
			baseCurrency
			acceptedCurrencies
		}
		outlets(propertyId: $propertyId) {
			code
			name
			active
		}
	}
`);

export const RoutingDocument = graphql(`
	query Routing($propertyId: UUID!, $reservationId: UUID!) {
		routing(propertyId: $propertyId, reservationId: $reservationId)
	}
`);

export type Folio = FoliosQuery['folios'][number];
export type FolioLine = Folio['lines'][number];
export type Payment = Folio['payments'][number];
export type Invoice = Folio['invoices'][number];
export type Balance = Folio['balances'][number];
export type FolioContext = FolioContextQuery;
export type Outlet = FolioContextQuery['outlets'][number];
export type PaymentMethod = components['schemas']['PaymentMethod'];
export type SignedLink = components['schemas']['SignedLink'];

/**
 * A reservation's folios. Not named by a server event as such: `events.ts` refetches it on `folio:<id>` for any
 * folio it holds. The account is part of the key, so a change of the reservation's account reads its folio.
 */
export function foliosKey(propertyId: string, reservationId: string, accountId: string | null) {
	return ['folios', propertyId, reservationId, accountId] as const;
}

/** An account's folio (its city ledger) in the property; refetched on its `folio:<id>` event like a reservation's. */
export function accountFoliosKey(propertyId: string, accountId: string) {
	return ['folios', propertyId, 'account', accountId] as const;
}

/** The prefix of every folios key: of the property's, or of every property's. */
export function allFoliosKey(propertyId?: string) {
	return propertyId ? (['folios', propertyId] as const) : (['folios'] as const);
}

/** The base currency, accepted currencies and outlets folio forms offer; not named by any event. */
export function folioContextKey(propertyId: string) {
	return ['folio-context', propertyId] as const;
}

/** Under `reservation:<id>`, which the server queues when the routing changes. */
export function routingKey(reservationId: string) {
	return [...reservationKey(reservationId), 'routing'] as const;
}

export async function fetchFolios(
	propertyId: string,
	reservationId: string,
	signal?: AbortSignal
): Promise<Folio[]> {
	return (await query(FoliosDocument, { propertyId, reservationId }, signal)).folios;
}

/** The account's folio in the property, with its newest 500 lines and payments: empty until it has one. */
export async function fetchAccountFolios(
	propertyId: string,
	accountId: string,
	signal?: AbortSignal
): Promise<Folio[]> {
	return (await query(FoliosDocument, { propertyId, accountId }, signal)).folios;
}

export async function fetchFolioContext(
	propertyId: string,
	signal?: AbortSignal
): Promise<FolioContext> {
	return query(FolioContextDocument, { propertyId }, signal);
}

export async function fetchRouting(
	propertyId: string,
	reservationId: string,
	signal?: AbortSignal
): Promise<string[]> {
	return (await query(RoutingDocument, { propertyId, reservationId }, signal)).routing;
}

/** Minor units as digits in `currency`, with thousands separators and a leading minus: `-123450` USD is `-1,234.50`. */
export function formatAmount(minor: number, currency: string): string {
	const digits = exponent(currency);
	const sign = minor < 0 ? '-' : '';
	const text = String(Math.abs(minor)).padStart(digits + 1, '0');
	const whole = text.slice(0, text.length - digits).replace(/\B(?=(\d{3})+$)/g, ',');
	return digits === 0 ? sign + whole : `${sign}${whole}.${text.slice(text.length - digits)}`;
}

/** An amount with its currency: `USD 1,234.50`. */
export function money(minor: number, currency: string): string {
	return `${currency} ${formatAmount(minor, currency)}`;
}

/**
 * What a person typed, as minor units of `currency`, by exact digit arithmetic: `1,234.50` USD is `123450`.
 * Commas are accepted only as thousands separators, so `12,50` is refused rather than read as `1250`. `null` for
 * anything that is not a non-negative amount with at most the currency's decimals. Zero is returned as `0`:
 * callers that need more than zero (payments, refunds) refuse it themselves.
 */
export function parseAmount(text: string, currency: string): number | null {
	const digits = exponent(currency);
	const match = /^(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d*))?$/.exec(text.trim());
	if (!match) return null;
	const fraction = match[2] ?? '';
	if (fraction.length > digits) return null;
	const minor = Number(match[1].replaceAll(',', '') + fraction.padEnd(digits, '0'));
	return Number.isSafeInteger(minor) ? minor : null;
}

/** A balance with its base-currency equivalent when it is in another currency: `USD 10.00 (LKR 3,000.00)`. */
export function balanceText(balance: Balance, baseCurrency: string): string {
	const amount = money(balance.amount, balance.currency);
	if (balance.currency === baseCurrency) return amount;
	return `${amount} (${money(balance.baseAmount, baseCurrency)})`;
}

/** What the balance means: owed, in credit, or settled. */
export function balanceState(balance: Balance): 'due' | 'credit' | 'settled' {
	return balance.amount > 0 ? 'due' : balance.amount < 0 ? 'credit' : 'settled';
}

/** Every currency of the folio is at zero. */
export function isSettled(folio: Folio): boolean {
	return folio.balances.every((balance) => balance.amount === 0);
}

/** Why the folio cannot be invoiced now, or `null` when it can. */
export function invoiceBlocker(folio: Folio): string | null {
	if (folio.status === 'INVOICED')
		return 'Already invoiced. A credit note on its invoice reopens it.';
	if (folio.linesOmitted) return 'Open the account to invoice its ledger.';
	if (folio.lines.length === 0) return 'Nothing to invoice: the folio has no charges.';
	const open = folio.balances.filter((balance) => balance.amount !== 0);
	if (open.length > 0) {
		const owed = open
			.map((balance) =>
				balance.amount > 0
					? `${money(balance.amount, balance.currency)} due`
					: `${money(-balance.amount, balance.currency)} in credit`
			)
			.join(', ');
		return `Settle the balance first: ${owed}.`;
	}
	return null;
}

/** A reservation room as the folio tabs name it. */
export interface FolioRoom {
	id: string;
	room?: { number: string } | null;
	roomType: { code: string };
}

/**
 * Each folio's tab name: the deposit, each stay by its room (or room type when unassigned, numbered when two
 * share it), and the account by name.
 */
export function folioLabels(
	folios: readonly Folio[],
	rooms: readonly FolioRoom[],
	accountName: string | null
): Map<string, string> {
	const labels = new Map<string, string>();
	const stays = new Map<string, string>();
	const unassigned = new Map<string, number>();
	for (const room of rooms) {
		if (room.room) {
			stays.set(room.id, `Room ${room.room.number}`);
		} else {
			const count = (unassigned.get(room.roomType.code) ?? 0) + 1;
			unassigned.set(room.roomType.code, count);
			stays.set(room.id, `${room.roomType.code} · unassigned${count > 1 ? ` ${count}` : ''}`);
		}
	}
	for (const folio of folios) {
		if (folio.kind === 'DEPOSIT') labels.set(folio.id, 'Deposit');
		else if (folio.kind === 'ACCOUNT')
			labels.set(folio.id, accountName ? `Account · ${accountName}` : 'Account');
		else labels.set(folio.id, stays.get(folio.reservationRoomId ?? '') ?? 'Stay');
	}
	return labels;
}

/** The ids of the lines a reversal reverses. */
export function reversedLines(lines: readonly FolioLine[]): Set<string> {
	return new Set(lines.flatMap((line) => (line.reverses ? [line.reverses] : [])));
}

/** A line can be reversed or transferred once, and a reversal never. */
export function canCorrect(line: FolioLine, reversed: ReadonlySet<string>): boolean {
	return !line.reverses && !reversed.has(line.id);
}

/**
 * The other side of a transferred line: the folio holding the other line with its `transferId`. A reversal
 * moved its charge there (`to`); a plain line came from there (`from`).
 */
export function transferCounterpart(
	line: FolioLine,
	folioId: string,
	folios: readonly Folio[]
): { direction: 'to' | 'from'; folioId: string } | null {
	if (!line.transferId) return null;
	for (const folio of folios) {
		for (const other of folio.lines) {
			if (other.id !== line.id && other.transferId === line.transferId && folio.id !== folioId) {
				return { direction: line.reverses ? 'to' : 'from', folioId: folio.id };
			}
		}
	}
	return null;
}

/** What is left of a payment to refund: its amount less the refunds of it, on whichever folio they were made. */
export function refundable(payment: Payment, folios: readonly Folio[]): number {
	if (payment.amount <= 0 || payment.method === 'account' || payment.method === 'transfer')
		return 0;
	const refunded = folios
		.flatMap((folio) => folio.payments)
		.filter((other) => other.refundOf === payment.id)
		.reduce((sum, other) => sum + other.amount, 0);
	return Math.max(0, payment.amount + refunded);
}

const METHOD_LABELS: Record<string, string> = {
	cash: 'Cash',
	card_terminal: 'Card',
	bank_transfer: 'Bank transfer',
	account: 'Charged to account',
	transfer: 'Moved'
};

export function methodLabel(method: string): string {
	return METHOD_LABELS[method] ?? method;
}

/** The staff-chosen methods, in the order the payment form offers them. */
export const PAYMENT_METHODS: readonly PaymentMethod[] = [
	'cash',
	'card_terminal',
	'bank_transfer',
	'account'
];

function text(details: unknown, key: string): string | null {
	if (typeof details !== 'object' || details === null) return null;
	const value = (details as Record<string, unknown>)[key];
	return typeof value === 'string' && value ? value : null;
}

/** A payment's details as one line: a card's brand, last 4 and approval, a transfer's reference, a refund's reason. */
export function paymentDetails(payment: Payment, labels: ReadonlyMap<string, string>): string {
	const { details } = payment;
	const parts: string[] = [];
	const brand = text(details, 'brand');
	const last4 = text(details, 'last4');
	if (brand || last4) parts.push([brand, last4 ? `····${last4}` : null].filter(Boolean).join(' '));
	const approval = text(details, 'approval');
	if (approval) parts.push(`approval ${approval}`);
	const reference = text(details, 'reference');
	if (reference) parts.push(`ref. ${reference}`);
	const to = text(details, 'to_folio');
	if (to) parts.push(`to ${labels.get(to) ?? 'another folio'}`);
	const from = text(details, 'from_folio');
	if (from) parts.push(`from ${labels.get(from) ?? 'another folio'}`);
	const reason = text(details, 'reason');
	if (reason) parts.push(reason);
	return parts.join(' · ');
}

/** The categories a charge or a routing rule can name, with the property's active outlets. */
export function categories(outlets: readonly Outlet[]): { value: string; label: string }[] {
	return [
		{ value: 'room', label: 'Room' },
		{ value: 'meal_plan', label: 'Meal plan' },
		{ value: 'laundry', label: 'Laundry' },
		{ value: 'misc', label: 'Miscellaneous' },
		{ value: 'fee', label: 'Fee' },
		...outlets
			.filter((outlet) => outlet.active)
			.map((outlet) => ({ value: `pos:${outlet.code}`, label: `${outlet.name} (POS)` }))
	];
}

/** A category as the UI names it; an outlet that is gone by its code. */
export function categoryLabel(category: string, outlets: readonly Outlet[]): string {
	const known = categories(outlets).find((entry) => entry.value === category);
	if (known) return known.label;
	return category.startsWith('pos:') ? `${category.slice(4)} (POS)` : category;
}

/** A tax's rate in percent from basis points: 1000 is `10%`, 250 is `2.5%`. */
export function ratePercent(rateBp: number): string {
	return `${rateBp / 100}%`;
}

/** Whose permission each folio action needs; the API enforces them. */
export interface FolioPermissions {
	/** `FolioPost`: post a charge, take a payment. */
	post: boolean;
	/** `FolioAdjust`: reverse, transfer, refund, routing, credit notes. */
	adjust: boolean;
	/** `InvoiceIssue`: issue an invoice, open its PDF. */
	invoice: boolean;
}

/** Which of a folio's own actions are offered. A folio read without its lines (the account's) offers none. */
export function folioActions(folio: Folio, can: FolioPermissions) {
	const open = folio.status === 'OPEN' && !folio.linesOmitted;
	return {
		postCharge: can.post && open,
		takePayment: can.post && open,
		correct: can.adjust && open,
		refund: can.adjust && open,
		issueInvoice: can.invoice && !folio.linesOmitted,
		viewInvoice: can.invoice,
		creditNote: can.adjust
	};
}

/** Invoices of the folio that no credit note credits yet. */
export function uncredited(folio: Folio): Set<string> {
	const credited = new Set(folio.invoices.flatMap((invoice) => invoice.credits ?? []));
	return new Set(
		folio.invoices
			.filter((invoice) => invoice.kind === 'INVOICE' && !credited.has(invoice.id))
			.map((invoice) => invoice.id)
	);
}

/** A fresh, audited 5-minute link to the invoice's PDF; never cached. */
export async function invoiceLink(propertyId: string, invoiceId: string): Promise<SignedLink> {
	return unwrap(
		await rest.GET('/api/v1/properties/{property}/invoices/{invoice}/link', {
			params: { path: { property: propertyId, invoice: invoiceId } }
		})
	);
}
