import { describe, expect, it } from 'vitest';
import { groupRooms, moveItem, rangeNumbers, roomTypesKey, roomsKey } from './rooms';

const room = (id: string, number: string, roomTypeId: string, floor: string | null) => ({
	id,
	number,
	roomTypeId,
	floor,
	sectionId: null,
	active: true,
	sortOrder: 0,
	version: 1
});

describe('moveItem', () => {
	it('moves an item to a new position without changing the input', () => {
		const items = ['a', 'b', 'c', 'd'];

		expect(moveItem(items, 0, 2)).toEqual(['b', 'c', 'a', 'd']);
		expect(moveItem(items, 3, 0)).toEqual(['d', 'a', 'b', 'c']);
		expect(items).toEqual(['a', 'b', 'c', 'd']);
	});

	it('ignores moves past either end', () => {
		expect(moveItem(['a', 'b'], 0, -1)).toEqual(['a', 'b']);
		expect(moveItem(['a', 'b'], 1, 2)).toEqual(['a', 'b']);
	});
});

describe('groupRooms', () => {
	const types = [
		{ id: 'std', code: 'STD', name: 'Standard' },
		{ id: 'dlx', code: 'DLX', name: 'Deluxe' }
	];
	const rooms = [
		room('1', '201', 'dlx', '2'),
		room('2', '101', 'std', '1'),
		room('3', 'G1', 'std', null)
	];

	it('groups by room type in type order, keeping room order within a group', () => {
		const groups = groupRooms(rooms, types, 'type');

		expect(groups.map((g) => g.label)).toEqual(['STD · Standard', 'DLX · Deluxe']);
		expect(groups[0].rooms.map((r) => r.number)).toEqual(['101', 'G1']);
	});

	it('groups by floor, rooms without a floor last', () => {
		const groups = groupRooms(rooms, types, 'floor');

		expect(groups.map((g) => g.label)).toEqual(['Floor 1', 'Floor 2', 'No floor']);
	});
});

describe('rangeNumbers', () => {
	it('lists the numbers a range will create', () => {
		expect(rangeNumbers('', 101, 104)).toEqual(['101', '102', '103', '104']);
		expect(rangeNumbers('A', 1, 2)).toEqual(['A1', 'A2']);
	});

	it('is empty for a backwards range', () => {
		expect(rangeNumbers('', 5, 4)).toEqual([]);
	});
});

describe('query keys', () => {
	it('match the server event keys', () => {
		expect(roomTypesKey('p1')).toEqual(['room-types:p1']);
		expect(roomsKey('p1')).toEqual(['rooms:p1']);
	});
});
