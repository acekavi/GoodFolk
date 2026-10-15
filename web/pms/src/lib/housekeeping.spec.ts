import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
import { ApiError, NETWORK_ERROR } from './api/problem';
import { applyEvent } from './events';
import {
	conditionLabel,
	filterRooms,
	filtersFromSearchParams,
	filtersToSearchParams,
	hkKey,
	initials,
	issueCounts,
	isConditionRefusal,
	issuePhotosKey,
	issuesKey,
	moveTile,
	nextConditions,
	NO_FILTERS,
	occupancyLabel,
	overPhotoLimit,
	photoControl,
	rangeBetween,
	removePhotoMessage,
	stepLabel,
	type BoardRoom,
	type RoomIssue
} from './housekeeping';

function room(over: Partial<BoardRoom> & { number: string }): BoardRoom {
	return {
		roomId: `id-${over.number}`,
		floor: '1',
		sectionId: null,
		roomTypeId: 't',
		condition: 'DIRTY',
		conditionVersion: 0,
		occupancy: 'VACANT',
		assignee: null,
		blockKind: null,
		...over
	};
}

describe('keys', () => {
	it('are the server event keys', () => {
		expect(hkKey('p1')).toEqual(['hk:p1']);
		expect(issuesKey('p1')).toEqual(['issues:p1']);
	});
});

describe('nextConditions', () => {
	it('lets a supervisor choose any other condition', () => {
		expect(nextConditions('supervisor', 'CLEAN')).toEqual(['DIRTY', 'CLEANING', 'INSPECTED']);
		expect(nextConditions('supervisor')).toHaveLength(4);
	});

	it('lets a housekeeper only advance dirty, then cleaning', () => {
		expect(nextConditions('housekeeper', 'DIRTY')).toEqual(['CLEANING']);
		expect(nextConditions('housekeeper', 'CLEANING')).toEqual(['CLEAN']);
		expect(nextConditions('housekeeper', 'CLEAN')).toEqual([]);
		expect(nextConditions('housekeeper', 'INSPECTED')).toEqual([]);
	});
});

describe('labels', () => {
	it('read as words', () => {
		expect(conditionLabel('INSPECTED')).toBe('Inspected');
		expect(occupancyLabel('STAY_OVER')).toBe('Stay-over');
		expect(occupancyLabel('DUE_OUT')).toBe('Due out');
	});

	it('gives initials of at most two words', () => {
		expect(initials('Kumari Silva')).toBe('KS');
		expect(initials('  nimal ')).toBe('N');
		expect(initials('Ann Marie Jones')).toBe('AM');
	});
});

describe('filters', () => {
	const rooms = [
		room({ number: '101', floor: '1', sectionId: 's1', condition: 'CLEAN' }),
		room({
			number: '102',
			floor: '1',
			assignee: { userId: 'u1', displayName: 'Kumari Silva' },
			occupancy: 'DUE_OUT'
		}),
		room({ number: '201', floor: '2', sectionId: 's1' }),
		room({ number: 'X', floor: null })
	];
	const numbers = (filters: Partial<typeof NO_FILTERS>) =>
		filterRooms(rooms, { ...NO_FILTERS, ...filters }).map((r) => r.number);

	it('keeps everything without filters', () => {
		expect(numbers({})).toEqual(['101', '102', '201', 'X']);
	});

	it('combines floor, section, housekeeper, condition and occupancy', () => {
		expect(numbers({ floor: '2' })).toEqual(['201']);
		expect(numbers({ floor: 'none' })).toEqual(['X']);
		expect(numbers({ section: 's1' })).toEqual(['101', '201']);
		expect(numbers({ housekeeper: 'u1' })).toEqual(['102']);
		expect(numbers({ housekeeper: 'none' })).toEqual(['101', '201', 'X']);
		expect(numbers({ condition: 'CLEAN' })).toEqual(['101']);
		expect(numbers({ occupancy: 'DUE_OUT' })).toEqual(['102']);
		expect(numbers({ floor: '1', section: 's1' })).toEqual(['101']);
	});

	it('round-trips through the URL, dropping unknown values', () => {
		const filters = {
			floor: '2',
			section: 's1',
			housekeeper: 'u1',
			condition: 'CLEAN' as const,
			occupancy: 'DUE_OUT' as const
		};
		const params = filtersToSearchParams(filters);
		expect(params.toString()).toBe('floor=2&section=s1&hk=u1&condition=clean&occupancy=due_out');
		expect(filtersFromSearchParams(params)).toEqual(filters);
		expect(filtersFromSearchParams(new URLSearchParams('condition=bogus'))).toEqual(NO_FILTERS);
		expect(filtersToSearchParams(NO_FILTERS).toString()).toBe('');
	});
});

describe('selection helpers', () => {
	it('counts open issues per room', () => {
		const issues = [{ roomId: 'a' }, { roomId: 'a' }, { roomId: 'b' }] as RoomIssue[];
		expect(issueCounts(issues)).toEqual(
			new Map([
				['a', 2],
				['b', 1]
			])
		);
	});

	it('moves between tiles by arrows, pages and row ends, staying on the board', () => {
		// 10 tiles, 4 to a row: the last row holds two.
		const move = (index: number, key: string, ctrl = false) => moveTile(index, key, ctrl, 4, 10, 2);
		expect(move(5, 'ArrowLeft')).toBe(4);
		expect(move(5, 'ArrowRight')).toBe(6);
		expect(move(5, 'ArrowUp')).toBe(1);
		expect(move(5, 'ArrowDown')).toBe(9);
		expect(move(7, 'ArrowDown')).toBe(9);
		expect(move(0, 'ArrowLeft')).toBe(0);
		expect(move(9, 'ArrowRight')).toBe(9);
		expect(move(1, 'ArrowUp')).toBe(0);
		expect(move(6, 'Home')).toBe(4);
		expect(move(5, 'End')).toBe(7);
		expect(move(9, 'End')).toBe(9);
		expect(move(8, 'End')).toBe(9);
		expect(move(6, 'Home', true)).toBe(0);
		expect(move(1, 'End', true)).toBe(9);
		expect(move(1, 'PageDown')).toBe(9);
		expect(move(9, 'PageUp')).toBe(1);
		expect(move(1, 'a')).toBeNull();
	});

	it('spans a shift-click range in either direction', () => {
		const order = ['a', 'b', 'c', 'd'];
		expect(rangeBetween(order, 'b', 'd')).toEqual(['b', 'c', 'd']);
		expect(rangeBetween(order, 'd', 'b')).toEqual(['b', 'c', 'd']);
		expect(rangeBetween(order, undefined, 'c')).toEqual(['c']);
		expect(rangeBetween(order, 'gone', 'c')).toEqual(['c']);
		expect(rangeBetween(order, 'a', 'gone')).toEqual([]);
	});
});

describe('isConditionRefusal', () => {
	it('is a 409 that names the room and a condition that is not ready', () => {
		expect(isConditionRefusal(409, 'room 101 is Dirty')).toBe(true);
		expect(isConditionRefusal(409, 'room 7B is Cleaning')).toBe(true);
		expect(isConditionRefusal(409, 'room Suite 12 is Dirty')).toBe(true);
		expect(isConditionRefusal(409, 'room Suite 12 is Dirty and more')).toBe(false);
	});

	it('is nothing else', () => {
		expect(isConditionRefusal(403, 'room 101 is Dirty')).toBe(false);
		expect(isConditionRefusal(409, 'room 101 is Clean')).toBe(false);
		expect(
			isConditionRefusal(409, 'only a confirmed stay can check in; this one is Cancelled')
		).toBe(false);
		expect(isConditionRefusal(409, undefined)).toBe(false);
	});
});

describe('stepLabel', () => {
	it("names a housekeeper's two steps and a supervisor's other settings", () => {
		expect(stepLabel('CLEANING')).toBe('Start cleaning');
		expect(stepLabel('CLEAN')).toBe('Done');
		expect(stepLabel('INSPECTED')).toBe('Set inspected');
		expect(stepLabel('DIRTY')).toBe('Set dirty');
	});
});

describe('photoControl', () => {
	it('needs a connection and a free place among five', () => {
		expect(photoControl(true, 0)).toEqual({ disabled: false, note: '' });
		expect(photoControl(true, 4)).toEqual({ disabled: false, note: '' });
		expect(photoControl(true, 5)).toEqual({
			disabled: true,
			note: 'An issue can have at most 5 photos'
		});
		expect(photoControl(false, 0)).toEqual({ disabled: true, note: 'Photos need a connection' });
	});
});

describe('overPhotoLimit', () => {
	it('refuses the files chosen at once beyond the fifth place', () => {
		const reasons = [0, 1, 2].map((index) => overPhotoLimit(3, index));
		expect(reasons).toEqual([null, null, 'An issue can have at most 5 photos']);
	});
});

describe('removePhotoMessage', () => {
	const refusal = (status: number, detail?: string) =>
		new ApiError({ type: 'about:blank', title: 'Refused', status, detail });

	it('explains a 403 as someone else’s photo', () => {
		expect(removePhotoMessage(refusal(403, 'Forbidden'))).toBe(
			'Only the person who added this photo or a supervisor can remove it'
		);
	});

	it('passes other problems on', () => {
		expect(removePhotoMessage(refusal(404, 'photo not found'))).toBe('photo not found');
		expect(removePhotoMessage(new TypeError('Failed to fetch'))).toBe(NETWORK_ERROR);
	});
});

describe('issuePhotosKey', () => {
	it('is refetched by the server’s media:<property> event, not by issues:', () => {
		const client = new QueryClient();
		client.setQueryData(issuePhotosKey('p1'), {});

		applyEvent(client, 'invalidate', '["issues:p1"]');
		expect(client.getQueryState(issuePhotosKey('p1'))?.isInvalidated).toBe(false);

		applyEvent(client, 'invalidate', '["media:p1"]');
		expect(client.getQueryState(issuePhotosKey('p1'))?.isInvalidated).toBe(true);
	});
});
