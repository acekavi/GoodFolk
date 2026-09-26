import { graphql } from './api/gql';
import type { RateGridQuery, RatePlansQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { addDays, monthDays } from './inventory';

export const RatePlansDocument = graphql(`
	query RatePlans($propertyId: UUID!) {
		ratePlans(propertyId: $propertyId) {
			id
			code
			name
			kind
			segment
			residency
			currency
			parentId
			depth
			deriveMode
			deriveValue
			roundingStep
			extraAdultAmount
			inheritRestrictions
			allowedMealPlans
			cancellationPolicyId
			roomTypeIds
			active
			version
		}
		mealSupplements(propertyId: $propertyId) {
			id
			mealPlan
			currency
			adultAmount
			childAmount
			from
			to
			version
		}
		cancellationPolicies(propertyId: $propertyId) {
			id
			name
			version
		}
	}
`);

export const RateGridDocument = graphql(`
	query RateGrid($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!) {
		rateGrid(propertyId: $propertyId, ratePlanId: $ratePlanId, from: $from, to: $to) {
			prices {
				roomTypeId
				date
				occupancy
				amount
			}
			restrictions {
				roomTypeId
				date
				closed
				minStay
				maxStay
				closedToArrival
				closedToDeparture
			}
		}
	}
`);

export const BulkPreviewDocument = graphql(`
	query BulkPreview(
		$propertyId: UUID!
		$ratePlanId: UUID!
		$from: Date!
		$to: Date!
		$weekdays: [Int!]!
		$roomTypeIds: [UUID!]!
		$mode: PriceChangeMode!
		$value: Int!
	) {
		bulkChangePreview(
			propertyId: $propertyId
			ratePlanId: $ratePlanId
			from: $from
			to: $to
			weekdays: $weekdays
			roomTypeIds: $roomTypeIds
			mode: $mode
			value: $value
		) {
			total
			cells {
				roomTypeId
				date
				occupancy
				before
				after
			}
		}
	}
`);

export const QuoteDocument = graphql(`
	query Quote(
		$propertyId: UUID!
		$roomTypeId: UUID!
		$ratePlanId: UUID!
		$mealPlan: MealPlan!
		$checkIn: Date!
		$checkOut: Date!
		$adults: Int!
		$children: Int!
		$residency: Residency!
	) {
		quote(
			propertyId: $propertyId
			roomTypeId: $roomTypeId
			ratePlanId: $ratePlanId
			mealPlan: $mealPlan
			checkIn: $checkIn
			checkOut: $checkOut
			adults: $adults
			children: $children
			residency: $residency
		) {
			nights {
				date
				room
				meal
			}
			total
			currency
			restrictionsOk
			violations {
				kind
				date
				message
			}
		}
	}
`);

export type RatePlan = RatePlansQuery['ratePlans'][number];
export type MealSupplement = RatePlansQuery['mealSupplements'][number];
export type RateGrid = RateGridQuery['rateGrid'];
export type Restriction = RateGrid['restrictions'][number];

/** Plans, meal supplements and cancellation policies change together under `rate-plans:<property>`. */
export function ratePlansKey(propertyId: string) {
	return [`rate-plans:${propertyId}`] as const;
}

/** Query key shared with the server's `rates:<property>:<plan>:<yyyy-mm>` event. */
export function ratesKey(propertyId: string, planId: string, month: string) {
	return [`rates:${propertyId}:${planId}:${month}`] as const;
}

export async function fetchRatePlans(propertyId: string, signal?: AbortSignal) {
	return query(RatePlansDocument, { propertyId }, signal);
}

/** A plan's prices and restrictions for a month (`YYYY-MM`). */
export async function fetchRateMonth(
	propertyId: string,
	ratePlanId: string,
	month: string,
	signal?: AbortSignal
) {
	const days = monthDays(month);
	const to = addDays(days[days.length - 1], 1);
	return (await query(RateGridDocument, { propertyId, ratePlanId, from: days[0], to }, signal))
		.rateGrid;
}

/** Digits after the decimal point in `currency` (2 for USD and LKR, 0 for JPY). */
function minorDigits(currency: string): number {
	return new Intl.NumberFormat('en', { style: 'currency', currency }).resolvedOptions()
		.maximumFractionDigits!;
}

/** Minor units as a plain number in `currency`, e.g. `15050` USD as `150.50`. */
export function formatMoney(amount: number, currency: string): string {
	const digits = minorDigits(currency);
	return new Intl.NumberFormat('en', {
		minimumFractionDigits: digits,
		maximumFractionDigits: digits
	}).format(amount / 10 ** digits);
}

/** What a person typed as minor units of `currency`, rounded half-up; `null` if it is not an amount. */
export function parseMoney(text: string, currency: string): number | null {
	const cleaned = text.replaceAll(',', '').trim();
	if (!/^\d+(\.\d*)?$/.test(cleaned)) return null;
	const digits = minorDigits(currency);
	const [whole, fraction = ''] = cleaned.split('.');
	const kept = fraction.padEnd(digits, '0').slice(0, digits);
	const roundUp = fraction.length > digits && Number(fraction[digits]) >= 5;
	return Number(whole) * 10 ** digits + Number(kept || '0') + (roundUp ? 1 : 0);
}

export interface RateRow {
	id: string;
	label: string;
	roomTypeId: string;
	code: string;
	/** Adults; `null` for the room type's restrictions row. */
	occupancy: number | null;
}

/** Grid rows: each active room type the plan sells, by occupancy (1 to its maximum adults), then its restrictions. */
export function rateRows(
	plan: Pick<RatePlan, 'roomTypeIds'>,
	roomTypes: readonly { id: string; code: string; maxAdults: number; active: boolean }[]
): RateRow[] {
	return roomTypes
		.filter((type) => type.active && plan.roomTypeIds.includes(type.id))
		.flatMap((type) => [
			...Array.from({ length: type.maxAdults }, (_, index) => ({
				id: `${type.id}:${index + 1}`,
				label: `${type.code} · ${index + 1} adult${index === 0 ? '' : 's'}`,
				roomTypeId: type.id,
				code: type.code,
				occupancy: index + 1
			})),
			{
				id: `${type.id}:restrictions`,
				label: `${type.code} · restrictions`,
				roomTypeId: type.id,
				code: type.code,
				occupancy: null
			}
		]);
}

/** Looks up a month's prices and restrictions. */
export function indexRates(grid: RateGrid) {
	const prices = new Map(
		grid.prices.map((p) => [`${p.roomTypeId}|${p.date}|${p.occupancy}`, p.amount])
	);
	const restrictions = new Map(grid.restrictions.map((r) => [`${r.roomTypeId}|${r.date}`, r]));
	return {
		price: (roomTypeId: string, date: string, occupancy: number) =>
			prices.get(`${roomTypeId}|${date}|${occupancy}`),
		restriction: (roomTypeId: string, date: string) => restrictions.get(`${roomTypeId}|${date}`)
	};
}

/**
 * Every plan derived from `id`, directly or through another derived plan, found by following
 * `parentId` links. Does not include `id` itself.
 */
export function descendants(
	plans: readonly Pick<RatePlan, 'id' | 'parentId'>[],
	id: string
): Set<string> {
	const children = new Map<string, string[]>();
	for (const plan of plans) {
		if (!plan.parentId) continue;
		children.set(plan.parentId, [...(children.get(plan.parentId) ?? []), plan.id]);
	}
	const found = new Set<string>();
	const queue = [...(children.get(id) ?? [])];
	while (queue.length > 0) {
		const next = queue.shift()!;
		if (found.has(next)) continue;
		found.add(next);
		queue.push(...(children.get(next) ?? []));
	}
	return found;
}

/** How a derived plan follows its parent, e.g. `BAR + 15%, rounded to 1.00`. */
export function formula(
	plan: Pick<RatePlan, 'deriveMode' | 'deriveValue' | 'roundingStep' | 'currency'>,
	parent: Pick<RatePlan, 'code'>
): string {
	const value = plan.deriveValue ?? 0;
	const sign = value < 0 ? '−' : '+';
	const size =
		plan.deriveMode === 'PERCENT'
			? `${Math.abs(value) / 100}%`
			: formatMoney(Math.abs(value), plan.currency);
	const rounding =
		plan.roundingStep > 1 ? `, rounded to ${formatMoney(plan.roundingStep, plan.currency)}` : '';
	return `${parent.code} ${sign} ${size}${rounding}`;
}

/** The restrictions set on a day, e.g. `Closed · Min 2 · CTA`; empty when there are none. */
export function restrictionSummary(
	r: Pick<Restriction, 'closed' | 'minStay' | 'maxStay' | 'closedToArrival' | 'closedToDeparture'>
): string {
	return [
		r.closed && 'Closed',
		r.minStay && `Min ${r.minStay}`,
		r.maxStay && `Max ${r.maxStay}`,
		r.closedToArrival && 'CTA',
		r.closedToDeparture && 'CTD'
	]
		.filter(Boolean)
		.join(' · ');
}

/**
 * Collects edits and saves them together once none has come for `delay` ms, so cells edited in quick
 * succession go out in one request. A cell edited twice is saved with its last value. A failed save goes to
 * `onError`; its edits are not retried (the screen refetches and shows what was saved).
 *
 * Saves are serialized: a flush's `save` does not start until the previous flush's `save` has settled
 * (succeeded or failed). The prices endpoint is unversioned (last write wins), so if a cell were re-edited
 * while its save was still in flight, two overlapping PUTs could arrive out of order and let the stale one
 * win; queuing each flush's save behind the one before it rules that out.
 */
export function batcher<T>(
	key: (item: T) => string,
	save: (items: T[]) => Promise<void>,
	delay = 400,
	onError: (err: unknown) => void = () => {}
) {
	let queued = new Map<string, T>();
	let saving = new Set<string>();
	let timer: ReturnType<typeof setTimeout> | undefined;
	/** Resolves once every flush queued so far has run its `save` and settled, in order. */
	let chain: Promise<void> = Promise.resolve();

	async function flush() {
		clearTimeout(timer);
		const items = queued;
		queued = new Map();
		if (items.size === 0) return;
		saving = new Set([...saving, ...items.keys()]);
		const previous = chain;
		const run = (async () => {
			// A previous flush's `run` only rejects if its own `onError` callback itself threw; swallow
			// that here so a broken `onError` can't deadlock every flush queued after it.
			await previous.catch(() => {});
			try {
				await save([...items.values()]);
			} catch (err) {
				onError(err);
			} finally {
				for (const itemKey of items.keys()) saving.delete(itemKey);
			}
		})();
		chain = run;
		await run;
	}

	return {
		add(item: T) {
			queued.set(key(item), item);
			clearTimeout(timer);
			timer = setTimeout(flush, delay);
		},
		/** Whether an edit of this cell is waiting to be saved or being saved. */
		has(itemKey: string) {
			return queued.has(itemKey) || saving.has(itemKey);
		},
		flush
	};
}
