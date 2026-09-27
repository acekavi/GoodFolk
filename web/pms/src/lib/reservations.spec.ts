import { describe, expect, it } from 'vitest';
import {
	availabilityKey,
	filterFromSearchParams,
	filterToSearchParams,
	formatStay,
	groupOffers,
	offerLabel,
	reservationKey,
	reservationsKey,
	statusLabel,
	violationsText,
	type ReservationListParams,
	type RoomTypeAvailability
} from './reservations';

describe('keys', () => {
	it("the reservations list key starts with the server's reservations:<property> event key", () => {
		const key = reservationsKey('p1', {
			filter: { text: 'smith' },
			sort: { field: 'GUEST', direction: 'DESC' }
		});
		expect(key[0]).toBe('reservations:p1');
		expect(key).toEqual([
			'reservations:p1',
			{ filter: { text: 'smith' }, sort: { field: 'GUEST', direction: 'DESC' } }
		]);
	});

	it('the reservations key still starts with the same string with no params, so a bare event still matches', () => {
		expect(reservationsKey('p1')[0]).toBe('reservations:p1');
	});

	it("the reservation detail key matches the server's reservation:<id> event key exactly", () => {
		expect(reservationKey('r1')).toEqual(['reservation:r1']);
	});

	it('the availability key carries every argument that changes the answer', () => {
		expect(availabilityKey('p1', '2026-10-03', '2026-10-05', 2, 1, 'RESIDENT')).toEqual([
			'availability',
			'p1',
			'2026-10-03',
			'2026-10-05',
			2,
			1,
			'RESIDENT'
		]);
	});
});

describe('search params round trip', () => {
	it('round-trips every filter and sort field', () => {
		const params: ReservationListParams = {
			filter: {
				arrivalFrom: '2026-10-01',
				arrivalTo: '2026-10-31',
				statuses: ['CONFIRMED', 'CHECKED_IN'],
				sources: ['FRONT_DESK'],
				text: 'GFK-0001'
			},
			sort: { field: 'GUEST', direction: 'DESC' }
		};

		const search = filterToSearchParams(params);

		expect(filterFromSearchParams(search)).toEqual(params);
	});

	it('omits fields at their default and produces no search string for the default list', () => {
		const search = filterToSearchParams({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});

		expect(search.toString()).toBe('');
		expect(filterFromSearchParams(new URLSearchParams())).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('keeps an explicitly empty selection distinct from no filter at all', () => {
		const params: ReservationListParams = {
			filter: { statuses: [], sources: [] },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		};

		const search = filterToSearchParams(params);

		expect(search.get('statuses')).toBe('');
		expect(filterFromSearchParams(search)).toEqual(params);
	});

	it('writes params in a stable order regardless of the object key order given to it', () => {
		const a = filterToSearchParams({
			filter: { text: 'x', arrivalFrom: '2026-01-01' },
			sort: { field: 'CREATED', direction: 'DESC' }
		});
		const b = filterToSearchParams({
			filter: { arrivalFrom: '2026-01-01', text: 'x' },
			sort: { direction: 'DESC', field: 'CREATED' }
		});

		expect(a.toString()).toBe(b.toString());
		expect([...a.keys()]).toEqual(['arrivalFrom', 'text', 'sort', 'dir']);
	});

	it('ignores params it does not recognise', () => {
		const search = new URLSearchParams('foo=bar&arrivalFrom=2026-10-01&utm_source=x');

		expect(filterFromSearchParams(search)).toEqual({
			filter: { arrivalFrom: '2026-10-01' },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('drops unknown enum values from statuses, keeping the known ones', () => {
		const search = new URLSearchParams('statuses=CONFIRMED,BOGUS,CHECKED_IN');

		expect(filterFromSearchParams(search)).toEqual({
			filter: { statuses: ['CONFIRMED', 'CHECKED_IN'] },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('falls back to default sort field when the value is unknown', () => {
		const search = new URLSearchParams('sort=NOPE');

		expect(filterFromSearchParams(search)).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('falls back to default sort direction when the value is unknown', () => {
		const search = new URLSearchParams('dir=SIDEWAYS');

		expect(filterFromSearchParams(search)).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});
});

describe('formatStay', () => {
	it('shows the year once, at the end, within one calendar year', () => {
		expect(formatStay('2026-10-03', '2026-10-05')).toBe('3 Oct – 5 Oct 2026 · 2 nights');
	});

	it('crosses months within the same year', () => {
		expect(formatStay('2026-09-28', '2026-10-03')).toBe('28 Sep – 3 Oct 2026 · 5 nights');
	});

	it('carries the year on both ends when the stay crosses a new year', () => {
		expect(formatStay('2026-12-30', '2027-01-02')).toBe('30 Dec 2026 – 2 Jan 2027 · 3 nights');
	});

	it('says "1 night" in the singular', () => {
		expect(formatStay('2026-10-03', '2026-10-04')).toBe('3 Oct – 4 Oct 2026 · 1 night');
	});
});

describe('statusLabel', () => {
	it('reads every room status in plain words', () => {
		expect(statusLabel('TENTATIVE')).toBe('Tentative');
		expect(statusLabel('CHECKED_IN')).toBe('Checked in');
		expect(statusLabel('NO_SHOW')).toBe('No-show');
	});
});

describe('violationsText', () => {
	it("joins every violation's message the way the server joins a 422", () => {
		expect(violationsText([{ message: 'closed' }, { message: 'below minimum stay' }])).toBe(
			'closed; below minimum stay'
		);
		expect(violationsText([])).toBe('');
	});
});

describe('offerLabel', () => {
	it('names the plan and the meal plan', () => {
		expect(offerLabel({ ratePlanCode: 'BAR', mealPlan: 'HB' })).toBe('BAR · Half board');
	});
});

describe('groupOffers', () => {
	it('flattens each room type into one row per offer, marking what cannot be sold', () => {
		const availability: RoomTypeAvailability[] = [
			{
				roomTypeId: 'dlx',
				code: 'DLX',
				name: 'Deluxe',
				free: 0,
				offers: [
					{
						ratePlanId: 'bar',
						ratePlanCode: 'BAR',
						mealPlan: 'RO',
						total: 10000,
						currency: 'USD',
						restrictionsOk: true,
						violations: [],
						nights: []
					}
				]
			},
			{
				roomTypeId: 'std',
				code: 'STD',
				name: 'Standard',
				free: 3,
				offers: [
					{
						ratePlanId: 'bar',
						ratePlanCode: 'BAR',
						mealPlan: 'BB',
						total: 12000,
						currency: 'USD',
						restrictionsOk: false,
						violations: [{ kind: 'CLOSED', message: 'closed' }],
						nights: []
					}
				]
			}
		];

		const rows = groupOffers(availability);

		expect(rows).toHaveLength(2);
		expect(rows[0]).toMatchObject({
			roomTypeId: 'dlx',
			label: 'BAR · Room only',
			totalLabel: '100.00',
			sellable: false // no rooms free, even though restrictionsOk
		});
		expect(rows[1]).toMatchObject({
			roomTypeId: 'std',
			sellable: false,
			violations: 'closed'
		});
	});
});
