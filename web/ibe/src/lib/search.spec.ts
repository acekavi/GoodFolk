import { describe, expect, it, vi } from 'vitest';
import type { SiteRoomType } from './api';
import {
	canAddRoom,
	fitsSomeRoom,
	movedTo,
	occupancyLimits,
	parseRooms,
	pickerSteps,
	searchFromUrl,
	stayOf,
	stayProblem,
	type SearchForm
} from './search';

function roomType(max_adults: number, max_children: number, max_occupancy: number): SiteRoomType {
	return {
		id: `rt-${max_adults}-${max_children}-${max_occupancy}`,
		name: 'Room',
		description: '',
		beds: [],
		amenities: [],
		max_adults,
		max_children,
		max_occupancy,
		media: []
	};
}

const BUSINESS_DATE = '2026-11-01';
const form = (patch: Partial<SearchForm> = {}): SearchForm => ({
	checkIn: '2026-11-10',
	checkOut: '2026-11-13',
	residency: 'non_resident',
	rooms: [{ adults: 2, children: 0 }],
	...patch
});

describe('the occupancy picker', () => {
	const types = [roomType(2, 1, 3), roomType(3, 0, 3)];

	it('allows the most adults and children any one room type holds', () => {
		expect(occupancyLimits(types)).toEqual({ maxAdults: 3, maxChildren: 1 });
	});

	it('adds a guest only while some room type still holds the room', () => {
		expect(pickerSteps(types, { adults: 2, children: 0 })).toEqual({
			addAdult: true,
			removeAdult: true,
			addChild: true,
			removeChild: false
		});
		// Three adults fit only the type with no children; two adults and a child fit only the other.
		expect(pickerSteps(types, { adults: 3, children: 0 }).addChild).toBe(false);
		expect(pickerSteps(types, { adults: 2, children: 1 }).addAdult).toBe(false);
	});

	it('keeps at least one adult in a room', () => {
		expect(pickerSteps(types, { adults: 1, children: 1 }).removeAdult).toBe(false);
	});

	it('respects a room type’s total occupancy, not just its adults and children', () => {
		expect(fitsSomeRoom([roomType(2, 2, 3)], { adults: 2, children: 2 })).toBe(false);
		expect(fitsSomeRoom([roomType(2, 2, 3)], { adults: 2, children: 1 })).toBe(true);
	});

	it('allows 1 to 4 rooms', () => {
		expect(canAddRoom([{ adults: 2, children: 0 }])).toBe(true);
		expect(canAddRoom(Array.from({ length: 4 }, () => ({ adults: 1, children: 0 })))).toBe(false);
		expect(parseRooms('2-0,2-1,1-0,1-0,1-0')).toBeNull();
		expect(parseRooms('0-1')).toBeNull();
		expect(parseRooms('2-0,2-1')).toEqual([
			{ adults: 2, children: 0 },
			{ adults: 2, children: 1 }
		]);
	});
});

describe('the stay limits', () => {
	it('takes 1 to 30 nights from the business date up to 18 months ahead', () => {
		expect(stayProblem(form(), BUSINESS_DATE)).toBeNull();
		expect(
			stayProblem(form({ checkIn: BUSINESS_DATE, checkOut: '2026-11-02' }), BUSINESS_DATE)
		).toBeNull();
		expect(stayProblem(form({ checkIn: '2026-10-31' }), BUSINESS_DATE)).toBe(
			'Check-in cannot be in the past'
		);
		expect(stayProblem(form({ checkOut: '2026-11-10' }), BUSINESS_DATE)).toBe(
			'Check-out must be after check-in'
		);
		expect(stayProblem(form({ checkOut: '2026-12-11' }), BUSINESS_DATE)).toBe(
			'A stay can be up to 30 nights'
		);
		expect(
			stayProblem(form({ checkIn: '2028-05-02', checkOut: '2028-05-03' }), BUSINESS_DATE)
		).toBe('Check-in can be up to 18 months ahead');
	});
});

describe('no availability', () => {
	it('tries the same nights from the next available check-in', () => {
		const stay = {
			checkIn: '2026-11-10',
			checkOut: '2026-11-13',
			residency: 'resident' as const,
			rooms: []
		};

		expect(movedTo(stay, '2026-11-15')).toMatchObject({
			checkIn: '2026-11-15',
			checkOut: '2026-11-18'
		});
	});
});

describe('the residency gate', () => {
	it('gives no stay to search before the residency question is answered', () => {
		expect(stayOf(form({ residency: null }), BUSINESS_DATE)).toBeNull();
		expect(stayProblem(form({ residency: null }), BUSINESS_DATE)).toBe(
			'Answer “Are you a resident of Sri Lanka?” to see prices'
		);
		expect(stayOf(form({ residency: 'resident' }), BUSINESS_DATE)?.residency).toBe('resident');
	});

	it('sends no search request before an answer', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>();
		const url = new URL(
			'http://localhost:5174/GAL/rooms?check_in=2026-11-10&check_out=2026-11-13&rooms=2-0'
		);

		expect(await searchFromUrl(fetch, 'GAL', url, BUSINESS_DATE)).toEqual({ kind: 'unanswered' });
		url.searchParams.set('residency', 'maybe');
		expect(await searchFromUrl(fetch, 'GAL', url, BUSINESS_DATE)).toEqual({ kind: 'unanswered' });
		expect(fetch).not.toHaveBeenCalled();
	});

	it('searches once answered, with the answer in the query', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(
			async () =>
				new Response(JSON.stringify({ room_types: [], next_available: '2026-11-12' }), {
					status: 200,
					headers: { 'content-type': 'application/json' }
				})
		);
		const url = new URL(
			'http://localhost:5174/GAL/rooms?check_in=2026-11-10&check_out=2026-11-13&rooms=2-0,2-1&residency=resident'
		);

		const searched = await searchFromUrl(fetch, 'GAL', url, BUSINESS_DATE);

		expect(searched).toMatchObject({ kind: 'found', result: { next_available: '2026-11-12' } });
		expect(fetch).toHaveBeenCalledOnce();
		const sent = new URL((fetch.mock.calls[0][0] as Request).url);
		expect(sent.pathname).toBe('/api/public/v1/ibe/GAL/search');
		expect(Object.fromEntries(sent.searchParams)).toEqual({
			check_in: '2026-11-10',
			check_out: '2026-11-13',
			residency: 'resident',
			rooms: '2-0,2-1'
		});
	});

	it('tells a guest who searched too often to wait', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(
			async () =>
				new Response(JSON.stringify({ title: 'Too many requests', status: 429 }), {
					status: 429,
					headers: { 'content-type': 'application/problem+json', 'retry-after': '30' }
				})
		);
		const url = new URL(
			'http://localhost:5174/GAL/rooms?check_in=2026-11-10&check_out=2026-11-13&rooms=2-0&residency=resident'
		);

		expect(await searchFromUrl(fetch, 'GAL', url, BUSINESS_DATE)).toEqual({
			kind: 'refused',
			message: 'Too many tries; wait a minute'
		});
	});
});
