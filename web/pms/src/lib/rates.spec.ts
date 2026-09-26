import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	batcher,
	formatMoney,
	formula,
	indexRates,
	parseMoney,
	ratePlansKey,
	rateRows,
	ratesKey,
	restrictionSummary,
	type RatePlan
} from './rates';

const plan = (overrides: Partial<RatePlan> = {}): RatePlan => ({
	id: 'bar',
	code: 'BAR',
	name: 'Best available',
	kind: 'STANDARD',
	segment: 'IBE',
	residency: null,
	currency: 'USD',
	parentId: null,
	depth: 0,
	deriveMode: null,
	deriveValue: null,
	roundingStep: 100,
	extraAdultAmount: 0,
	inheritRestrictions: false,
	allowedMealPlans: ['RO'],
	cancellationPolicyId: null,
	roomTypeIds: ['dlx'],
	active: true,
	version: 1,
	...overrides
});

describe('keys', () => {
	it('match the server events', () => {
		expect(ratePlansKey('p1')).toEqual(['rate-plans:p1']);
		expect(ratesKey('p1', 'bar', '2026-07')).toEqual(['rates:p1:bar:2026-07']);
	});
});

describe('money', () => {
	it("formats minor units with the currency's decimals", () => {
		expect(formatMoney(15050, 'USD')).toBe('150.50');
		expect(formatMoney(4500000, 'LKR')).toBe('45,000.00');
		expect(formatMoney(1500, 'JPY')).toBe('1,500');
	});

	it('parses what a person types into minor units', () => {
		expect(parseMoney('150.5', 'USD')).toBe(15050);
		expect(parseMoney(' 45,000 ', 'LKR')).toBe(4500000);
		expect(parseMoney('0.005', 'USD')).toBe(1);
		expect(parseMoney('1500', 'JPY')).toBe(1500);
		expect(parseMoney('', 'USD')).toBeNull();
		expect(parseMoney('-5', 'USD')).toBeNull();
		expect(parseMoney('abc', 'USD')).toBeNull();
	});
});

describe('rateRows', () => {
	it('lists each sold room type by occupancy, then its restrictions', () => {
		const types = [
			{ id: 'std', code: 'STD', name: 'Standard', maxAdults: 1, active: true },
			{ id: 'dlx', code: 'DLX', name: 'Deluxe', maxAdults: 2, active: true }
		];

		const rows = rateRows(plan({ roomTypeIds: ['dlx'] }), types);

		expect(rows.map((row) => row.label)).toEqual([
			'DLX · 1 adult',
			'DLX · 2 adults',
			'DLX · restrictions'
		]);
		expect(rows[1]).toMatchObject({ id: 'dlx:2', roomTypeId: 'dlx', occupancy: 2, code: 'DLX' });
		expect(rows[2].occupancy).toBeNull();
	});
});

describe('indexRates', () => {
	it("finds a price and a day's restrictions", () => {
		const index = indexRates({
			prices: [{ roomTypeId: 'dlx', date: '2026-07-04', occupancy: 2, amount: 12000 }],
			restrictions: [
				{
					roomTypeId: 'dlx',
					date: '2026-07-04',
					closed: false,
					minStay: 2,
					maxStay: null,
					closedToArrival: true,
					closedToDeparture: false
				}
			]
		});

		expect(index.price('dlx', '2026-07-04', 2)).toBe(12000);
		expect(index.price('dlx', '2026-07-04', 1)).toBeUndefined();
		expect(index.restriction('dlx', '2026-07-04')?.minStay).toBe(2);
	});
});

describe('formula', () => {
	it('describes how a derived plan follows its parent', () => {
		const bar = plan();

		expect(formula(plan({ deriveMode: 'PERCENT', deriveValue: 1500 }), bar)).toBe(
			'BAR + 15%, rounded to 1.00'
		);
		expect(formula(plan({ deriveMode: 'AMOUNT', deriveValue: -1050, roundingStep: 1 }), bar)).toBe(
			'BAR − 10.50'
		);
	});
});

describe('restrictionSummary', () => {
	it('names each restriction that is set', () => {
		expect(
			restrictionSummary({
				closed: true,
				minStay: 2,
				maxStay: 7,
				closedToArrival: true,
				closedToDeparture: true
			})
		).toBe('Closed · Min 2 · Max 7 · CTA · CTD');
		expect(
			restrictionSummary({
				closed: false,
				minStay: null,
				maxStay: null,
				closedToArrival: false,
				closedToDeparture: false
			})
		).toBe('');
	});
});

describe('batcher', () => {
	afterEach(() => vi.useRealTimers());

	it('saves the edits made in a short burst together, the last edit of a cell winning', async () => {
		vi.useFakeTimers();
		const saved: string[][] = [];
		const batch = batcher<{ cell: string; value: string }>(
			(edit) => edit.cell,
			async (edits) => void saved.push(edits.map((edit) => `${edit.cell}=${edit.value}`)),
			400
		);

		batch.add({ cell: 'a', value: '1' });
		await vi.advanceTimersByTimeAsync(300);
		batch.add({ cell: 'b', value: '2' });
		batch.add({ cell: 'a', value: '3' });
		expect(batch.has('a')).toBe(true);
		await vi.advanceTimersByTimeAsync(400);

		expect(saved).toEqual([['a=3', 'b=2']]);
		expect(batch.has('a')).toBe(false);
	});

	it('hands a failed save to its caller and keeps nothing queued', async () => {
		vi.useFakeTimers();
		const failures: unknown[] = [];
		const batch = batcher<{ cell: string }>(
			(edit) => edit.cell,
			() => Promise.reject(new Error('offline')),
			400,
			(err) => failures.push(err)
		);

		batch.add({ cell: 'a' });
		await vi.advanceTimersByTimeAsync(400);

		expect(failures).toHaveLength(1);
		expect(batch.has('a')).toBe(false);
	});
});
