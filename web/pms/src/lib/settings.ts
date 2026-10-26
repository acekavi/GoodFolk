import { graphql } from './api/gql';
import type {
	BlockReasonsQuery,
	ExchangeRatesQuery,
	OutletsQuery,
	TaxSettingsQuery
} from './api/gql/graphql';
import { query } from './api/graphql';
import { roomsKey } from './rooms';
import { can, canAnywhere, posOnly, type Profile } from './session';

/** The Taxes page: the property's tax settings, every rule (ended or not), and the outlets a rule can name. */
export const TaxSettingsDocument = graphql(`
	query TaxSettings($propertyId: UUID!) {
		propertyTaxSettings(propertyId: $propertyId) {
			vatRegistered
			tin
			sltdaLicence
			tdlRegistrationNo
			vatReturnPeriod
			tdlSmallTurnover
			legalName
			address
			baseCurrency
			acceptedCurrencies
			version
		}
		taxRules(propertyId: $propertyId) {
			id
			code
			name
			kind
			rateBp
			appliesTo
			base
			sequence
			presentation
			condition
			effectiveFrom
			effectiveTo
			version
		}
		outlets(propertyId: $propertyId) {
			code
			name
			active
		}
	}
`);

/** The Outlets page. */
export const OutletsDocument = graphql(`
	query Outlets($propertyId: UUID!) {
		outlets(propertyId: $propertyId) {
			id
			code
			name
			serviceCharge
			active
			version
			reportGroup
		}
	}
`);

/** The Exchange rates page: the stored rates of `[from, to)`, the import's alert, and the currencies to show. */
export const ExchangeRatesDocument = graphql(`
	query ExchangeRates($propertyId: UUID!, $from: Date!, $to: Date!) {
		fxRates(propertyId: $propertyId, from: $from, to: $to) {
			date
			currency
			rate
			source
			sourceDate
		}
		fxAlert(propertyId: $propertyId) {
			message
			raisedAt
		}
		propertyTaxSettings(propertyId: $propertyId) {
			baseCurrency
			acceptedCurrencies
		}
	}
`);

/** The Block reasons page: every reason, retired or not, with the version a change sends. */
export const BlockReasonsDocument = graphql(`
	query BlockReasons($propertyId: UUID!) {
		blockReasons(propertyId: $propertyId) {
			id
			code
			label
			defaultKind
			active
			version
		}
	}
`);

/** The banner's alert alone. */
export const FxAlertDocument = graphql(`
	query FxAlert($propertyId: UUID!) {
		fxAlert(propertyId: $propertyId) {
			message
			raisedAt
		}
	}
`);

export type TaxSettings = TaxSettingsQuery['propertyTaxSettings'];
export type TaxRule = TaxSettingsQuery['taxRules'][number];
export type OutletRow = OutletsQuery['outlets'][number];
export type FxRate = ExchangeRatesQuery['fxRates'][number];
export type BlockReasonRow = BlockReasonsQuery['blockReasons'][number];

/** Not named by any server event: the settings commands invalidate it themselves. */
export function taxSettingsKey(propertyId: string) {
	return ['tax-settings', propertyId] as const;
}

/** Not named by any server event: the outlet commands invalidate it themselves. */
export function outletsKey(propertyId: string) {
	return ['outlets', propertyId] as const;
}

/** Under the server's `fx:<property>` event, which a new rate, an import or an alert queues. */
export function fxKey(propertyId: string) {
	return [`fx:${propertyId}`] as const;
}

export function exchangeRatesKey(propertyId: string, from: string, to: string) {
	return [...fxKey(propertyId), 'rates', from, to] as const;
}

export function fxAlertKey(propertyId: string) {
	return [...fxKey(propertyId), 'alert'] as const;
}

/** Under the server's `rooms:<property>` event, which a block reason change queues. */
export function blockReasonsKey(propertyId: string) {
	return [...roomsKey(propertyId), 'block-reasons'] as const;
}

export async function fetchBlockReasons(propertyId: string, signal?: AbortSignal) {
	return (await query(BlockReasonsDocument, { propertyId }, signal)).blockReasons;
}

export async function fetchTaxSettings(propertyId: string, signal?: AbortSignal) {
	return query(TaxSettingsDocument, { propertyId }, signal);
}

export async function fetchOutlets(propertyId: string, signal?: AbortSignal) {
	return (await query(OutletsDocument, { propertyId }, signal)).outlets;
}

export async function fetchExchangeRates(
	propertyId: string,
	from: string,
	to: string,
	signal?: AbortSignal
) {
	return query(ExchangeRatesDocument, { propertyId, from, to }, signal);
}

export async function fetchFxAlert(propertyId: string, signal?: AbortSignal) {
	return (await query(FxAlertDocument, { propertyId }, signal)).fxAlert;
}

/**
 * A rate typed as a percentage, with at most 2 decimals, as basis points by exact digit arithmetic: `18` is 1800,
 * `2.5` is 250, `0.05` is 5. `null` for anything else, or above 100%.
 */
export function percentToBp(text: string): number | null {
	const match = /^(\d{1,3})(?:\.(\d{1,2}))?$/.exec(text.trim());
	if (!match) return null;
	const bp = Number(match[1]) * 100 + Number((match[2] ?? '').padEnd(2, '0'));
	return bp <= 10_000 ? bp : null;
}

/** Basis points as the percentage `percentToBp` reads back, without trailing zeros: 250 is `2.5`, 1800 is `18`. */
export function bpToPercent(bp: number): string {
	const whole = Math.floor(bp / 100);
	const fraction = String(bp % 100)
		.padStart(2, '0')
		.replace(/0+$/, '');
	return fraction ? `${whole}.${fraction}` : String(whole);
}

/**
 * Whether `text` is an exchange rate the server takes: positive, at most 12 digits before the point and 6 after,
 * no sign, exponent or separators.
 */
export function isValidRate(text: string): boolean {
	const match = /^(\d{1,12})(?:\.(\d{1,6}))?$/.exec(text);
	return !!match && /[1-9]/.test(match[1] + (match[2] ?? ''));
}

/** A settings page the index links to; Sections and Cancellation policies live on Rooms and Rate plans. */
export type SettingsLink =
	| 'general'
	| 'taxes'
	| 'exchange-rates'
	| 'sections'
	| 'block-reasons'
	| 'outlets'
	| 'laundry-prices'
	| 'cancellation-policies'
	| 'staff'
	| 'permissions';

export interface SettingsGroup {
	group: string;
	links: { link: SettingsLink; label: string }[];
}

/**
 * The Settings index at the property: its groups, each with the pages the user may open, and no group left empty.
 * Each page keeps its own permission: General `PropertiesManage`; Taxes and invoices, Currencies and exchange rates
 * and Outlets `FolioView`; Sections and Block reasons `RoomsManage`; Laundry prices `LaundryPricesManage`;
 * Cancellation policies (on Rate plans) `RatesManage`; Staff `StaffManage` at any property; Permissions every member
 * but a till (`posOnly`).
 */
export function settingsIndex(profile: Profile, propertyId: string): SettingsGroup[] {
	const at = (action: Parameters<typeof can>[1]) => can(profile, action, propertyId);
	const groups: {
		group: string;
		links: { link: SettingsLink; label: string; shown: boolean }[];
	}[] = [
		{
			group: 'Property',
			links: [
				{ link: 'general', label: 'General', shown: at('propertiesManage') },
				{ link: 'taxes', label: 'Taxes and invoices', shown: at('folioView') },
				{
					link: 'exchange-rates',
					label: 'Currencies and exchange rates',
					shown: at('folioView')
				}
			]
		},
		{
			group: 'Operations',
			links: [
				{ link: 'sections', label: 'Sections', shown: at('manageRooms') },
				{ link: 'block-reasons', label: 'Block reasons', shown: at('manageRooms') },
				{ link: 'outlets', label: 'Outlets', shown: at('folioView') },
				{ link: 'laundry-prices', label: 'Laundry prices', shown: at('laundryPricesManage') }
			]
		},
		{
			group: 'Rates',
			links: [
				{
					link: 'cancellation-policies',
					label: 'Cancellation policies',
					shown: at('manageRates')
				}
			]
		},
		{
			group: 'Staff',
			links: [
				{ link: 'staff', label: 'Staff', shown: canAnywhere(profile, 'staffManage') },
				{
					link: 'permissions',
					label: 'Permissions',
					shown: profile.grants.length > 0 && !posOnly(profile)
				}
			]
		}
	];
	return groups
		.map(({ group, links }) => ({
			group,
			links: links.filter((entry) => entry.shown).map(({ link, label }) => ({ link, label }))
		}))
		.filter((group) => group.links.length > 0);
}

/** Whether the exchange-rate alert banner is shown: to those who can set a rate by hand. */
export function seesFxBanner(profile: Profile, propertyId: string): boolean {
	return can(profile, 'fxOverride', propertyId);
}

export interface RulePeriod {
	from: string;
	to: string | null;
	state: 'current' | 'upcoming' | 'ended';
	rules: TaxRule[];
}

/**
 * Rules grouped by their effective period, in calculation order within each: the periods in force on the
 * business date first, then those to come (soonest first), then those ended (latest first).
 */
export function rulePeriods(rules: readonly TaxRule[], businessDate: string): RulePeriod[] {
	const periods = new Map<string, RulePeriod>();
	for (const rule of rules) {
		const key = `${rule.effectiveFrom}/${rule.effectiveTo ?? ''}`;
		let period = periods.get(key);
		if (!period) {
			const state =
				rule.effectiveTo != null && rule.effectiveTo <= businessDate
					? 'ended'
					: rule.effectiveFrom > businessDate
						? 'upcoming'
						: 'current';
			period = { from: rule.effectiveFrom, to: rule.effectiveTo ?? null, state, rules: [] };
			periods.set(key, period);
		}
		period.rules.push(rule);
	}
	const rank = { current: 0, upcoming: 1, ended: 2 } as const;
	const list = [...periods.values()];
	for (const period of list) period.rules.sort((a, b) => a.sequence - b.sequence);
	return list.sort((a, b) => {
		if (a.state !== b.state) return rank[a.state] - rank[b.state];
		return a.state === 'ended' ? b.from.localeCompare(a.from) : a.from.localeCompare(b.from);
	});
}

/** A rule condition as the API takes and gives it. */
export type RuleCondition =
	| { type: 'always' }
	| { type: 'vat_registered' }
	| { type: 'residency'; resident: boolean }
	| { type: 'payment_currency'; currencies: string[] };

/** The condition read from the API's JSON; an unknown shape reads as `always`. */
export function readCondition(value: unknown): RuleCondition {
	const condition = (typeof value === 'object' && value !== null ? value : {}) as Record<
		string,
		unknown
	>;
	switch (condition.type) {
		case 'vat_registered':
			return { type: 'vat_registered' };
		case 'residency':
			return { type: 'residency', resident: condition.resident === true };
		case 'payment_currency':
			return {
				type: 'payment_currency',
				currencies: Array.isArray(condition.currencies)
					? condition.currencies.filter((c): c is string => typeof c === 'string')
					: []
			};
		default:
			return { type: 'always' };
	}
}

export function conditionLabel(value: unknown): string {
	const condition = readCondition(value);
	switch (condition.type) {
		case 'always':
			return 'Always';
		case 'vat_registered':
			return 'When VAT-registered';
		case 'residency':
			return condition.resident ? 'Residents only' : 'Non-residents only';
		case 'payment_currency':
			return `Paid in ${condition.currencies.join(', ')}`;
	}
}

const KIND_LABELS: Record<TaxRule['kind'], string> = {
	TAX: 'Tax',
	LEVY: 'Levy',
	SERVICE_CHARGE: 'Service charge'
};

export function kindLabel(kind: TaxRule['kind']): string {
	return KIND_LABELS[kind];
}

/** What a rule applies to, as the table shows it: `*` is everything, `pos:*` every outlet. */
export function appliesToLabel(pattern: string): string {
	if (pattern === '*') return 'Everything';
	if (pattern === 'pos:*') return 'All outlets';
	return pattern;
}

/** Accepted currencies as typed: comma- or space-separated codes, upper-cased, each once, in order. */
export function parseCurrencies(text: string): string[] {
	const codes = text
		.split(/[\s,]+/)
		.map((code) => code.trim().toUpperCase())
		.filter(Boolean);
	return [...new Set(codes)];
}
