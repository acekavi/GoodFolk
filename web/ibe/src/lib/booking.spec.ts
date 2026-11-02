import { describe, expect, it } from 'vitest';
import type { SearchResult } from './api';
import {
	attemptKeys,
	bookHref,
	bookingOutcome,
	bookingPayload,
	depositLines,
	depositText,
	handOver,
	pickParam,
	pricesAgree,
	picksFromUrl,
	pricePicks,
	takeBooked,
	totalsOf,
	type Details,
	type Pick
} from './booking';
import type { Stay } from './search';

const meal = (meal_plan: string, rooms: number[], total: number) => ({
	meal_plan,
	rooms,
	total,
	nights: [],
	taxes: []
});

/** Two rooms asked, 2+0 and 2+1: a USD plan on the deluxe and an LKR plan on the standard. */
const result: SearchResult = {
	next_available: null,
	room_types: [
		{
			room_type_id: 'dlx',
			available: 1,
			plans: [
				{
					rate_plan_id: 'bar-usd',
					name: 'Best available',
					currency: 'USD',
					cancellation: 'Free cancellation.',
					meal_plans: [meal('RO', [0], 30_000), meal('BB', [0], 36_000), meal('BB', [1], 42_000)]
				}
			]
		},
		{
			room_type_id: 'std',
			available: 3,
			plans: [
				{
					rate_plan_id: 'res-lkr',
					name: 'Residents',
					currency: 'LKR',
					cancellation: 'Free cancellation.',
					meal_plans: [meal('HB', [0, 1], 4_500_000)]
				}
			]
		}
	]
};

const stay: Stay = {
	checkIn: '2026-11-10',
	checkOut: '2026-11-13',
	residency: 'non_resident',
	rooms: [
		{ adults: 2, children: 0 },
		{ adults: 2, children: 1 }
	]
};

const picks: Pick[] = [
	{ roomTypeId: 'dlx', ratePlanId: 'bar-usd', mealPlan: 'BB' },
	{ roomTypeId: 'std', ratePlanId: 'res-lkr', mealPlan: 'HB' }
];

const details: Details = {
	firstName: ' Kamala ',
	lastName: 'Perera',
	email: 'kamala@example.com ',
	phone: '+94 77 123 4567',
	country: 'LK',
	arrivalTime: '',
	occupants: ['', 'Nimal Silva'],
	specialRequests: '  A quiet room, please. ',
	agreed: true
};

describe('the picks', () => {
	it('travel in the book page’s address, one per room in order', () => {
		const url = new URL('http://localhost:5174/GAL/book');
		for (const pick of picks) url.searchParams.append('pick', pickParam(pick));

		expect(picksFromUrl(url, 2)).toEqual(picks);
		expect(picksFromUrl(url, 3)).toBeNull();
		url.searchParams.append('pick', 'broken');
		expect(picksFromUrl(url, 3)).toBeNull();
	});

	it('make the book page’s address with the stay', () => {
		expect(bookHref('/GAL', stay, picks)).toBe(
			'/GAL/book?check_in=2026-11-10&check_out=2026-11-13&residency=non_resident&rooms=2-0%2C2-1' +
				'&pick=dlx%7Ebar-usd%7EBB&pick=std%7Eres-lkr%7EHB'
		);
	});

	it('are priced by the meal offer for each room’s own index', () => {
		const priced = pricePicks(result, picks);

		expect(priced?.map((room) => room.offer.total)).toEqual([36_000, 4_500_000]);
		// The deluxe BB for room 2 (with a child) is its own offer.
		const second = pricePicks(result, [picks[1], { ...picks[0] }]);
		expect(second?.map((room) => room.offer.total)).toEqual([4_500_000, 42_000]);
	});

	it('are no longer offered when the room went or more rooms are picked than are free', () => {
		expect(pricePicks(result, [{ ...picks[0], mealPlan: 'FB' }, picks[1]])).toBeNull();
		expect(pricePicks(result, [picks[0], { ...picks[0] }])).toBeNull();
	});
});

describe('the booking payload', () => {
	it('is built from the choices, with the expected total per currency', () => {
		const rooms = pricePicks(result, picks)!;

		const body = bookingPayload(stay, rooms, details, totalsOf(rooms), 'token-1', '');

		expect(body).toEqual({
			residency: 'non_resident',
			check_in: '2026-11-10',
			check_out: '2026-11-13',
			booker: {
				first_name: 'Kamala',
				last_name: 'Perera',
				email: 'kamala@example.com',
				phone: '+94 77 123 4567',
				country: 'LK'
			},
			rooms: [
				{
					room_type_id: 'dlx',
					rate_plan_id: 'bar-usd',
					meal_plan: 'BB',
					adults: 2,
					children: 0
				},
				{
					room_type_id: 'std',
					rate_plan_id: 'res-lkr',
					meal_plan: 'HB',
					adults: 2,
					children: 1,
					occupant_name: 'Nimal Silva'
				}
			],
			special_requests: 'A quiet room, please.',
			expected_totals: { USD: 36_000, LKR: 4_500_000 },
			agreed: true,
			turnstile_token: 'token-1',
			website: ''
		});
	});

	it('sends the arrival time when one is given', () => {
		const rooms = pricePicks(result, picks)!;

		const body = bookingPayload(stay, rooms, { ...details, arrivalTime: '14:30' }, {}, 't', '');

		expect(body.booker.arrival_time).toBe('14:30');
	});

	it('sums rooms of one currency into one total', () => {
		const rooms = pricePicks(result, [picks[1], picks[1]])!;

		expect(totalsOf(rooms)).toEqual({ LKR: 9_000_000 });
	});

	it('carries the new totals the guest confirmed after a price change, and the honeypot as found', () => {
		const rooms = pricePicks(result, picks)!;

		const body = bookingPayload(stay, rooms, details, { USD: 37_000, LKR: 4_500_000 }, 't', 'x');

		expect(body.expected_totals).toEqual({ USD: 37_000, LKR: 4_500_000 });
		expect(body.website).toBe('x');
	});
});

describe('the advisory deposit', () => {
	const withDeposit = (amount: number, due_date: string) => ({
		...meal('BB', [0, 1], 36_000),
		deposit: { amount, due_date }
	});
	const deposits: SearchResult = {
		next_available: null,
		room_types: [
			{
				room_type_id: 'dlx',
				available: 3,
				plans: [
					{ ...result.room_types[0].plans[0], meal_plans: [withDeposit(18_000, '2026-11-03')] },
					{
						...result.room_types[0].plans[0],
						rate_plan_id: 'early-usd',
						meal_plans: [withDeposit(12_000, '2026-11-01')]
					}
				]
			}
		]
	};

	it('is summed per currency and due on the earliest date, as the email says it', () => {
		const rooms = pricePicks(deposits, [
			{ roomTypeId: 'dlx', ratePlanId: 'bar-usd', mealPlan: 'BB' },
			{ roomTypeId: 'dlx', ratePlanId: 'early-usd', mealPlan: 'BB' }
		])!;

		expect(depositLines(rooms)).toEqual([
			{ currency: 'USD', amount: 30_000, dueDate: '2026-11-01' }
		]);
		expect(depositText(depositLines(rooms))).toBe(
			'Deposit: US$ 300.00 by Sun, 1 Nov 2026. The hotel will contact you about the deposit.'
		);
	});

	it('is not mentioned when no plan asks for one', () => {
		const rooms = pricePicks(result, picks)!;

		expect(depositLines(rooms)).toEqual([]);
		expect(depositText([])).toBeNull();
	});
});

describe('after a price change', () => {
	it('shows the per-room prices and the deposit only once a new search adds up to the new totals', () => {
		const searched = pricePicks(result, picks)!;
		const changed = { USD: 37_000, LKR: 4_500_000 };
		const refreshed = structuredClone(result);
		refreshed.room_types[0].plans[0].meal_plans[1].total = 37_000;

		expect(pricesAgree(searched, totalsOf(searched))).toBe(true);
		expect(pricesAgree(searched, changed)).toBe(false);
		expect(pricesAgree(pricePicks(refreshed, picks)!, changed)).toBe(true);
		expect(pricesAgree(searched, { USD: 36_000 })).toBe(false);
	});
});

describe('the booking answer', () => {
	it('is a booking with its manage link, or without one on a replay', () => {
		expect(bookingOutcome(201, { confirmation_no: 'GAL-000001', manage_url: 'm' }, null)).toEqual({
			kind: 'booked',
			confirmationNo: 'GAL-000001',
			manageUrl: 'm'
		});
		expect(bookingOutcome(201, { confirmation_no: 'GAL-000001' }, null)).toMatchObject({
			manageUrl: null
		});
	});

	it('tells a price change, a sold-out room, a limit and a bot check apart', () => {
		const problem = (status: number, extra: object) => ({ title: 'x', status, ...extra });
		expect(
			bookingOutcome(409, problem(409, { reason: 'price_changed', totals: { USD: 1 } }), null)
		).toEqual({ kind: 'price_changed', totals: { USD: 1 } });
		expect(bookingOutcome(409, problem(409, { reason: 'sold_out' }), null)).toEqual({
			kind: 'sold_out'
		});
		expect(bookingOutcome(429, problem(429, {}), '42')).toEqual({
			kind: 'too_many',
			retryAfter: 42
		});
		expect(
			bookingOutcome(
				400,
				problem(400, { detail: 'The bot check failed; reload the page and try again' }),
				null
			)
		).toEqual({ kind: 'bot_check' });
		expect(bookingOutcome(422, problem(422, { detail: 'too many guests' }), null)).toEqual({
			kind: 'invalid',
			message: 'too many guests'
		});
		expect(bookingOutcome(503, null, null)).toEqual({ kind: 'failed' });
	});
});

describe('the idempotency key', () => {
	const rooms = () => pricePicks(result, picks)!;
	const attempt = (token: string, change: Partial<Details> = {}) =>
		bookingPayload(stay, rooms(), { ...details, ...change }, totalsOf(rooms()), token, '');

	it('is kept for a retry of the same booking after a network failure, even with a fresh token', () => {
		let next = 0;
		const keys = attemptKeys(() => `k${++next}`);

		expect(keys.keyFor(attempt('token-1'))).toBe('k1');
		// No answer came back: the guest tries again, with the token the widget renewed.
		expect(keys.keyFor(attempt('token-2'))).toBe('k1');
	});

	it('is kept after a 5xx and ended by any answer below 500', () => {
		let next = 0;
		const keys = attemptKeys(() => `k${++next}`);

		expect(keys.keyFor(attempt('t'))).toBe('k1');
		keys.answered(503);
		expect(keys.keyFor(attempt('t'))).toBe('k1');
		keys.answered(409);
		expect(keys.keyFor(attempt('t2'))).toBe('k2');
		keys.answered(201);
		expect(keys.keyFor(attempt('t3'))).toBe('k3');
	});

	it('is new for a booking with other details', () => {
		let next = 0;
		const keys = attemptKeys(() => `k${++next}`);

		expect(keys.keyFor(attempt('t'))).toBe('k1');
		expect(keys.keyFor(attempt('t', { specialRequests: 'A late arrival' }))).toBe('k2');
	});

	it('is new for the re-confirmed price after a price change', () => {
		let next = 0;
		const keys = attemptKeys(() => `k${++next}`);
		keys.keyFor(attempt('t'));
		keys.answered(409);

		const confirmed = bookingPayload(
			stay,
			rooms(),
			details,
			{ USD: 37_000, LKR: 4_500_000 },
			't2',
			''
		);

		expect(keys.keyFor(confirmed)).toBe('k2');
	});
});

describe('the confirmation hand-over', () => {
	it('gives the booking just made once, and only for its own confirmation number', () => {
		const booked = {
			confirmationNo: 'GAL-000001',
			manageUrl: null,
			stay,
			rooms: [],
			totals: {},
			email: 'kamala@example.com'
		};
		handOver(booked);

		expect(takeBooked('GAL-000002')).toBeNull();
		handOver(booked);
		expect(takeBooked('GAL-000001')).toEqual(booked);
		expect(takeBooked('GAL-000001')).toBeNull();
	});
});
