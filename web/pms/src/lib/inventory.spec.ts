import { describe, expect, it } from 'vitest';
import {
	addDays,
	blocksOn,
	indexInventory,
	inventoryKey,
	monthDays,
	monthOf,
	shiftMonth
} from './inventory';

describe('month helpers', () => {
	it('lists every day of a month as YYYY-MM-DD', () => {
		const days = monthDays('2028-02');

		expect(days).toHaveLength(29);
		expect(days[0]).toBe('2028-02-01');
		expect(days.at(-1)).toBe('2028-02-29');
	});

	it('moves between months across years', () => {
		expect(shiftMonth('2026-12', 1)).toBe('2027-01');
		expect(shiftMonth('2026-01', -1)).toBe('2025-12');
		expect(monthOf('2026-09-24')).toBe('2026-09');
	});

	it('adds days across month ends', () => {
		expect(addDays('2026-09-30', 1)).toBe('2026-10-01');
		expect(addDays('2026-03-01', -1)).toBe('2026-02-28');
	});

	it('keys a month the way the server names its events', () => {
		expect(inventoryKey('p1', '2026-09')).toEqual(['inventory:p1:2026-09']);
	});
});

describe('indexInventory', () => {
	it('finds a room type on a day', () => {
		const day = {
			date: '2026-09-24',
			roomTypeId: 't1',
			physical: 5,
			sold: 0,
			outOfOrder: 1,
			available: 4
		};

		const index = indexInventory([day]);

		expect(index.get('t1', '2026-09-24')).toBe(day);
		expect(index.get('t1', '2026-09-25')).toBeUndefined();
	});
});

describe('blocksOn', () => {
	const block = (id: string, roomId: string, from: string, to: string) => ({
		id,
		roomId,
		from,
		to,
		kind: 'OUT_OF_ORDER' as const,
		reasonId: 'r',
		note: '',
		version: 1
	});

	it('includes the first day of a block but not the day it ends', () => {
		const blocks = [block('b1', 'r101', '2026-09-24', '2026-09-26')];

		expect(blocksOn(blocks, '2026-09-24')).toHaveLength(1);
		expect(blocksOn(blocks, '2026-09-25')).toHaveLength(1);
		expect(blocksOn(blocks, '2026-09-26')).toHaveLength(0);
	});

	it('keeps only the given rooms', () => {
		const blocks = [
			block('b1', 'r101', '2026-09-24', '2026-09-26'),
			block('b2', 'r201', '2026-09-24', '2026-09-26')
		];

		expect(blocksOn(blocks, '2026-09-24', new Set(['r201'])).map((b) => b.id)).toEqual(['b2']);
	});
});
