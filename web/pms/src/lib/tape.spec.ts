import { describe, expect, it } from 'vitest';
import type { Room, RoomType } from './rooms';
import {
	barsFor,
	barWidth,
	barX,
	lruVictims,
	openingStart,
	naturalCompare,
	pageKey,
	pageOf,
	railRooms,
	selectRooms,
	suggest,
	tapeEventTiles,
	tapeKey,
	tileEnd,
	tilesFor,
	tilesInMonth,
	tileStartFor,
	monthsOfTile,
	viewFromSearchParams,
	viewToSearchParams,
	type RailRoom,
	type TapeStay,
	type TapeView
} from './tape';

const types = [
	{ id: 'ty-dlx', code: 'DLX', name: 'Deluxe', sortOrder: 2 },
	{ id: 'ty-std', code: 'STD', name: 'Standard', sortOrder: 1 }
] as RoomType[];

function room(id: string, number: string, roomTypeId: string, sortOrder = 0, active = true) {
	return { id, number, roomTypeId, sortOrder, active } as Room;
}

const rail: RailRoom[] = railRooms(
	[
		room('r99', '99', 'ty-std'),
		room('r101', '101', 'ty-std'),
		room('r102', '102', 'ty-std'),
		room('r1010', '1010', 'ty-std'),
		room('r201', '201', 'ty-dlx'),
		room('r202', '202', 'ty-dlx'),
		room('rx', '300', 'ty-dlx', 0, false)
	],
	types
);

describe('tiles', () => {
	it('anchors tiles on the epoch Monday, 14 days apart', () => {
		expect(tileStartFor('2020-01-06')).toBe('2020-01-06');
		expect(tileStartFor('2020-01-19')).toBe('2020-01-06');
		expect(tileStartFor('2020-01-20')).toBe('2020-01-20');
	});

	it('cuts across a year end and before the epoch', () => {
		expect(tileStartFor('2025-12-31')).toBe('2025-12-29');
		expect(tileStartFor('2026-01-01')).toBe('2025-12-29');
		expect(tileStartFor('2026-01-11')).toBe('2025-12-29');
		expect(tileStartFor('2026-01-12')).toBe('2026-01-12');
		expect(tileStartFor('2020-01-05')).toBe('2019-12-23');
		expect(tileStartFor('2019-12-23')).toBe('2019-12-23');
		expect(tileStartFor('2019-12-22')).toBe('2019-12-09');
	});

	it('ends a tile exclusively 14 days on', () => {
		expect(tileEnd('2026-01-12')).toBe('2026-01-26');
	});

	it('lists the tiles a view intersects, with overscan in the scroll direction', () => {
		// 2026-01-22 .. 2026-01-28 spans tiles 01-12 and 01-26
		expect(tilesFor('2026-01-22', 7, 0, 1)).toEqual(['2026-01-12', '2026-01-26']);
		expect(tilesFor('2026-01-22', 7, 1, 1)).toEqual(['2026-01-12', '2026-01-26', '2026-02-09']);
		expect(tilesFor('2026-01-22', 7, 1, -1)).toEqual(['2025-12-29', '2026-01-12', '2026-01-26']);
		expect(tilesFor('2026-01-22', 7, 1, 0)).toEqual(['2026-01-12', '2026-01-26']);
		expect(tilesFor('2026-01-12', 14, 0, -1)).toEqual(['2026-01-12']);
		expect(tilesFor('2026-01-13', 14, 0, 1)).toEqual(['2026-01-12', '2026-01-26']);
	});

	it('names the months a tile touches', () => {
		expect(monthsOfTile('2026-01-12')).toEqual(['2026-01']);
		expect(monthsOfTile('2026-01-26')).toEqual(['2026-01', '2026-02']);
		expect(monthsOfTile('2025-12-29')).toEqual(['2025-12', '2026-01']);
	});

	it('finds the cached tiles in a month once each', () => {
		const cached = ['2026-01-12', '2026-02-09', '2026-01-26', '2026-01-26'];
		expect(tilesInMonth('2026-02', cached)).toEqual(['2026-02-09', '2026-01-26']);
		expect(tilesInMonth('2026-03', cached)).toEqual([]);
	});

	it('maps an event key to the cached tiles of that property and month only', () => {
		const cached = ['2026-01-12', '2026-01-26', '2026-03-09'];
		expect(tapeEventTiles('tape:p1:2026-02', 'p1', cached)).toEqual(['2026-01-26']);
		expect(tapeEventTiles('tape:p2:2026-02', 'p1', cached)).toEqual([]);
		expect(tapeEventTiles('inventory:p1:2026-02', 'p1', cached)).toEqual([]);
	});
});

describe('naturalCompare', () => {
	it('orders digit runs by value', () => {
		expect(naturalCompare('99', '101')).toBeLessThan(0);
		expect(naturalCompare('101', '1010')).toBeLessThan(0);
		expect(naturalCompare('A2', 'A10')).toBeLessThan(0);
		expect(naturalCompare('A10', 'A2')).toBeGreaterThan(0);
		expect(naturalCompare('101', '101')).toBe(0);
		expect(['1010', '99', '101'].sort(naturalCompare)).toEqual(['99', '101', '1010']);
	});
});

describe('room picker', () => {
	it('keeps active rooms in type, room and natural number order', () => {
		expect(rail.map((r) => r.number)).toEqual(['99', '101', '102', '1010', '201', '202']);
		expect(rail[0].typeCode).toBe('STD');
	});

	it('suggests a type by code or name', () => {
		expect(suggest('DLX', rail, types)).toContainEqual({
			kind: 'type',
			roomTypeId: 'ty-dlx',
			label: 'DLX Deluxe'
		});
		expect(suggest('standard', rail, types).some((c) => c.kind === 'type')).toBe(true);
	});

	it('suggests rooms by number prefix and caps at eight', () => {
		const rooms = suggest('10', rail, types).filter((c) => c.kind === 'room');
		expect(rooms.map((c) => c.label)).toEqual(['101', '102', '1010']);
		const many = Array.from({ length: 20 }, (_, i) => ({
			id: `m${i}`,
			number: `1${i}`,
			roomTypeId: 'ty-std',
			typeCode: 'STD'
		}));
		expect(suggest('1', many, types)).toHaveLength(8);
		expect(suggest('  ', rail, types)).toEqual([]);
	});

	it('normalises a range whichever way round it is typed', () => {
		const expected = { kind: 'range', from: '101', to: '120', label: '101-120' };
		expect(suggest('101-120', rail, types)).toContainEqual(expected);
		expect(suggest('120-101', rail, types)).toContainEqual(expected);
		expect(suggest('A9-A1', rail, types)).toContainEqual({
			kind: 'range',
			from: 'A1',
			to: 'A9',
			label: 'A1-A9'
		});
	});

	it('selects every room with no chips and the union without duplicates otherwise', () => {
		expect(selectRooms(rail, [])).toEqual(rail);
		const picked = selectRooms(rail, [
			{ kind: 'type', roomTypeId: 'ty-dlx', label: 'DLX' },
			{ kind: 'range', from: '100', to: '1000', label: '100-1000' },
			{ kind: 'room', roomId: 'r201', label: '201' }
		]);
		expect(picked.map((r) => r.number)).toEqual(['101', '102', '201', '202']);
	});

	it('selects a numeric range naturally, not as text', () => {
		const picked = selectRooms(rail, [{ kind: 'range', from: '99', to: '101', label: '99-101' }]);
		expect(picked.map((r) => r.number)).toEqual(['99', '101']);
	});
});

describe('pageOf', () => {
	const many: RailRoom[] = Array.from({ length: 25 }, (_, i) => ({
		id: `m${i}`,
		number: String(i),
		roomTypeId: 'ty-std',
		typeCode: 'STD'
	}));

	it('pages in tens', () => {
		const second = pageOf(many, 2);
		expect(second.pages).toBe(3);
		expect(second.rooms.map((r) => r.id)).toEqual(many.slice(10, 20).map((r) => r.id));
		expect(pageOf(many, 3).rooms).toHaveLength(5);
	});

	it('clamps a page beyond the last or below the first', () => {
		expect(pageOf(many, 9).page).toBe(3);
		expect(pageOf(many, 0).page).toBe(1);
		expect(pageOf(many, Number.NaN).page).toBe(1);
	});

	it('gives one page for no rooms', () => {
		expect(pageOf([], 4)).toEqual({ rooms: [], page: 1, pages: 1 });
	});
});

describe('view state in the URL', () => {
	const view: TapeView = {
		chips: [
			{ kind: 'type', roomTypeId: 'ty-dlx', label: 'DLX' },
			{ kind: 'room', roomId: 'r99', label: '99' },
			{ kind: 'range', from: '101', to: '102', label: '101-102' }
		],
		page: 1,
		span: 30,
		start: '2026-03-02'
	};

	it('round-trips', () => {
		const params = viewToSearchParams(view);
		expect(params.get('pick')).toBe('t:ty-dlx,r:r99,n:101-102');
		expect(viewFromSearchParams(params, '2026-01-01', rail, types)).toEqual(view);
	});

	it('round-trips a later page', () => {
		const many = Array.from({ length: 25 }, (_, i) => ({
			id: `m${i}`,
			number: String(i),
			roomTypeId: 'ty-std',
			typeCode: 'STD'
		}));
		const params = viewToSearchParams({ chips: [], page: 3, span: 7, start: '2026-03-02' });
		expect(viewFromSearchParams(params, '2026-01-01', many, types).page).toBe(3);
	});

	it('defaults an empty URL to the business date, 14 days, all rooms', () => {
		expect(viewFromSearchParams(new URLSearchParams(), '2026-09-29', rail, types)).toEqual({
			chips: [],
			page: 1,
			span: 14,
			start: '2026-09-29'
		});
	});

	it('drops unknown ids and malformed values silently', () => {
		const params = new URLSearchParams({
			pick: 't:nope,r:nope,r:r99,junk,n:1,t:ty-std',
			page: '7',
			span: '9',
			start: 'tomorrow'
		});
		const parsed = viewFromSearchParams(params, '2026-09-29', rail, types);
		expect(parsed.chips.map((c) => c.kind)).toEqual(['room', 'type']);
		expect(parsed.page).toBe(1);
		expect(parsed.span).toBe(14);
		expect(parsed.start).toBe('2026-09-29');
	});
});

describe('keys', () => {
	it('builds the page and query keys', () => {
		expect(pageKey(['a', 'b'])).toBe('a,b');
		expect(tapeKey('p1', '2026-01-12', 'a,b')).toEqual(['tape', 'p1', '2026-01-12', 'a,b']);
	});
});

describe('bars', () => {
	const stay = (id: string, version = 1, guestName = 'Ada'): TapeStay => ({
		id,
		reservationId: 'res',
		roomId: 'r101',
		roomTypeId: 'ty-std',
		start: '2026-01-17',
		end: '2026-01-22',
		status: 'confirmed',
		guestName,
		accountName: null,
		version
	});
	const block = {
		id: 'b1',
		roomId: 'r101',
		start: '2026-01-10',
		end: '2026-01-12',
		reason: 'Paint'
	};

	it('merges a stay and a block present in two tiles into one bar each', () => {
		const bars = barsFor([
			{ stays: [stay('s1'), stay('s2')], blocks: [block] },
			{ stays: [stay('s1')], blocks: [block] }
		]);
		expect(bars.map((b) => `${b.kind}:${b.id}`)).toEqual(['stay:s1', 'stay:s2', 'block:b1']);
	});

	it('keeps the newer version of a stay seen twice', () => {
		const bars = barsFor([
			{ stays: [stay('s1', 2, 'New')], blocks: [] },
			{ stays: [stay('s1', 1, 'Old')], blocks: [] }
		]);
		expect(bars).toHaveLength(1);
		expect(bars[0]).toMatchObject({ guestName: 'New', version: 2 });
	});

	it('places and sizes a bar in pixels, end exclusive', () => {
		expect(barX('2026-01-17', '2026-01-15', 40)).toBe(80);
		expect(barX('2026-01-13', '2026-01-15', 40)).toBe(-80);
		expect(barX('2026-01-01', '2025-12-31', 10)).toBe(10);
		expect(barWidth('2026-01-17', '2026-01-22', 40)).toBe(200);
		expect(barWidth('2026-01-17', '2026-01-17', 40)).toBe(0);
	});
});

describe('openingStart', () => {
	it('opens two days before the business date', () => {
		expect(openingStart('2026-10-02')).toBe('2026-09-30');
		expect(openingStart('2026-03-01')).toBe('2026-02-27');
	});

	it('is the start a URL without one opens at', () => {
		const view = viewFromSearchParams(new URLSearchParams(), openingStart('2026-10-02'), [], []);
		expect(view.start).toBe('2026-09-30');
	});
});

describe('lruVictims', () => {
	const entry = (key: string, updatedAt: number, busy = false) => ({ key, updatedAt, busy });

	it('removes the least recently updated beyond the limit', () => {
		const entries = [entry('a', 3), entry('b', 1), entry('c', 2), entry('d', 4)];
		expect(lruVictims(entries, 2)).toEqual(['b', 'c']);
	});

	it('removes nothing within the limit', () => {
		expect(lruVictims([entry('a', 1), entry('b', 2)], 2)).toEqual([]);
	});

	it('never removes a query in use or fetching, even when it is the oldest', () => {
		const entries = [entry('a', 1, true), entry('b', 2), entry('c', 3), entry('d', 4)];
		expect(lruVictims(entries, 2)).toEqual(['b', 'c']);
	});
});
