import { describe, expect, it } from 'vitest';
import {
	batchLocations,
	batchStatusText,
	cellText,
	countDifferences,
	countItems,
	moveLocations,
	stockGrid,
	type LinenItem,
	type LinenLocation
} from './linen';

const item = (id: string, code: string, active = true): LinenItem => ({
	id,
	code,
	name: code.toLowerCase(),
	active,
	version: 1
});

const location = (
	id: string,
	name: string,
	kind: LinenLocation['kind'],
	active = true
): LinenLocation => ({ id, name, kind, sectionId: null, active, version: 1 });

describe('stockGrid', () => {
	const items = [item('i2', 'TOWEL'), item('i1', 'SHEET'), item('i3', 'OLD', false)];
	const locations = [
		location('l2', 'Store', 'STORE'),
		location('l1', 'Pantry A', 'PANTRY'),
		location('l3', 'Closed', 'PANTRY', false)
	];

	it('lays out active items by code against active locations by name', () => {
		const grid = stockGrid(items, locations, []);

		expect(grid.columns.map((column) => column.id)).toEqual(['l1', 'l2']);
		expect(grid.rows.map((row) => row.item.id)).toEqual(['i1', 'i2']);
	});

	it('fills each cell from the stock, with its par and whether it is low', () => {
		const grid = stockGrid(items, locations, [
			{ locationId: 'l1', itemId: 'i2', quantity: 3, par: 8, low: true },
			{ locationId: 'l2', itemId: 'i2', quantity: 40, par: null, low: false }
		]);

		const towels = grid.rows[1].cells;
		expect(towels.map((cell) => [cell.location.id, cell.quantity, cell.par, cell.low])).toEqual([
			['l1', 3, 8, true],
			['l2', 40, null, false]
		]);
	});

	it('reads a cell the stock leaves out as none, with no par', () => {
		const grid = stockGrid(items, locations, []);

		expect(grid.rows[0].cells[0]).toMatchObject({ quantity: 0, par: null, low: false });
	});
});

describe('cellText', () => {
	it('shows the par after the quantity when there is one', () => {
		expect(cellText({ quantity: 3, par: 8 })).toBe('3 of 8');
		expect(cellText({ quantity: 40, par: null })).toBe('40');
	});
});

describe('moveLocations', () => {
	const locations = [
		location('s', 'Store', 'STORE'),
		location('p', 'Pantry', 'PANTRY'),
		location('l', 'Laundry', 'LAUNDRY'),
		location('v', 'Vendor', 'VENDOR'),
		location('x', 'Closed store', 'STORE', false)
	];
	const ids = (list: LinenLocation[]) => list.map((entry) => entry.id);

	it('receives into an active store or pantry, from nowhere', () => {
		const { from, to } = moveLocations('receive', locations);
		expect(ids(from)).toEqual([]);
		expect(ids(to)).toEqual(['s', 'p']);
	});

	it('transfers between active stores and pantries', () => {
		const { from, to } = moveLocations('transfer', locations);
		expect(ids(from)).toEqual(['s', 'p']);
		expect(ids(to)).toEqual(['s', 'p']);
	});

	it('writes off from any active location, to nowhere', () => {
		const { from, to } = moveLocations('write_off', locations);
		expect(ids(from)).toEqual(['s', 'p', 'l', 'v']);
		expect(ids(to)).toEqual([]);
	});

	it('sends a batch from a store or pantry to a laundry or vendor', () => {
		const { from, to } = batchLocations(locations);
		expect(ids(from)).toEqual(['s', 'p']);
		expect(ids(to)).toEqual(['l', 'v']);
	});
});

describe('countDifferences', () => {
	const lines = [
		{ itemId: 'a', outstanding: 10 },
		{ itemId: 'b', outstanding: 4 },
		{ itemId: 'c', outstanding: 0 }
	];

	it('takes each count from what was still out, leaving out lines with nothing out', () => {
		expect(countDifferences(lines, { a: 8, b: 4 })).toEqual([
			{ itemId: 'a', outstanding: 10, counted: 8, difference: -2 },
			{ itemId: 'b', outstanding: 4, counted: 4, difference: 0 }
		]);
	});

	it('counts an item left out as none came back', () => {
		expect(countDifferences(lines, {})[1]).toMatchObject({ counted: 0, difference: -4 });
	});
});

describe('countItems', () => {
	it('sends only the items counted above zero', () => {
		expect(countItems({ a: 3, b: 0 })).toEqual([{ item_id: 'a', quantity: 3 }]);
	});
});

describe('batchStatusText', () => {
	it('reads a batch as sent, received with some outstanding, or settled', () => {
		expect(batchStatusText({ status: 'SENT', outstanding: 10 })).toBe('Sent');
		expect(batchStatusText({ status: 'RECEIVED', outstanding: 2 })).toBe('Received, 2 outstanding');
		expect(batchStatusText({ status: 'RECEIVED', outstanding: 0 })).toBe('Settled');
	});
});
