import { graphql } from './api/gql';
import type { ExchangeRatesQuery, OutletsQuery, TaxSettingsQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { can, type Profile } from './session';

/** The Taxes page: the property's tax settings, every rule (ended or not), and the outlets a rule can name. */
export const TaxSettingsDocument = graphql(`
	query TaxSettings($propertyId: UUID!) {
		propertyTaxSettings(propertyId: $propertyId) {
			vatRegistered
			tin
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

export type SettingsPage = 'taxes' | 'outlets' | 'exchange-rates';

/**
 * The settings pages a user sees at the property, and whether each can be changed there. Each is read with
 * `FolioView`; taxes and outlets are changed with `TaxSettingsManage`, exchange rates with `FxOverride`.
 */
export function settingsAccess(
	profile: Profile,
	propertyId: string
): { page: SettingsPage; label: string; edit: boolean }[] {
	if (!can(profile, 'folioView', propertyId)) return [];
	const taxes = can(profile, 'taxSettingsManage', propertyId);
	return [
		{ page: 'taxes', label: 'Taxes', edit: taxes },
		{ page: 'outlets', label: 'Outlets', edit: taxes },
		{
			page: 'exchange-rates',
			label: 'Exchange rates',
			edit: can(profile, 'fxOverride', propertyId)
		}
	];
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
