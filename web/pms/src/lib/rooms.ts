import { graphql } from './api/gql';
import type { RoomsQuery, RoomTypesQuery } from './api/gql/graphql';
import { query } from './api/graphql';

export const RoomTypesDocument = graphql(`
	query RoomTypes($propertyId: UUID!) {
		roomTypes(propertyId: $propertyId) {
			id
			code
			name
			baseOccupancy
			maxAdults
			maxChildren
			maxOccupancy
			overbooking
			amenities
			sortOrder
			active
			version
		}
	}
`);

export const RoomsDocument = graphql(`
	query Rooms($propertyId: UUID!) {
		rooms(propertyId: $propertyId) {
			id
			roomTypeId
			number
			floor
			sectionId
			active
			sortOrder
			version
		}
		sections(propertyId: $propertyId) {
			id
			name
			version
		}
		blockReasons(propertyId: $propertyId) {
			id
			code
			label
			defaultKind
			active
		}
	}
`);

export type RoomType = RoomTypesQuery['roomTypes'][number];
export type Room = RoomsQuery['rooms'][number];
export type Section = RoomsQuery['sections'][number];
export type BlockReason = RoomsQuery['blockReasons'][number];

/** Query keys shared with the server's events, so an `invalidate` refetches exactly these. */
export function roomTypesKey(propertyId: string) {
	return [`room-types:${propertyId}`] as const;
}

/** Rooms, sections and block reasons change together under the `rooms:<property>` event. */
export function roomsKey(propertyId: string) {
	return [`rooms:${propertyId}`] as const;
}

export async function fetchRoomTypes(propertyId: string, signal?: AbortSignal) {
	return (await query(RoomTypesDocument, { propertyId }, signal)).roomTypes;
}

export async function fetchRooms(propertyId: string, signal?: AbortSignal) {
	return query(RoomsDocument, { propertyId }, signal);
}

/** `items` with the one at `from` moved to `to`; unchanged if either is out of range. */
export function moveItem<T>(items: readonly T[], from: number, to: number): T[] {
	const moved = [...items];
	if (from < 0 || from >= items.length || to < 0 || to >= items.length) return moved;
	const [item] = moved.splice(from, 1);
	moved.splice(to, 0, item);
	return moved;
}

export interface RoomGroup {
	key: string;
	label: string;
	rooms: Room[];
}

/** Rooms grouped by type (in the types' display order) or by floor (rooms without a floor last). */
export function groupRooms(
	rooms: readonly Room[],
	roomTypes: readonly Pick<RoomType, 'id' | 'code' | 'name'>[],
	by: 'type' | 'floor'
): RoomGroup[] {
	if (by === 'type') {
		return roomTypes
			.map((type) => ({
				key: type.id,
				label: `${type.code} · ${type.name}`,
				rooms: rooms.filter((room) => room.roomTypeId === type.id)
			}))
			.filter((group) => group.rooms.length > 0);
	}
	const floors = [...new Set(rooms.map((room) => room.floor ?? ''))].sort((a, b) =>
		a === '' ? 1 : b === '' ? -1 : a.localeCompare(b, undefined, { numeric: true })
	);
	return floors.map((floor) => ({
		key: floor,
		label: floor ? `Floor ${floor}` : 'No floor',
		rooms: rooms.filter((room) => (room.floor ?? '') === floor)
	}));
}

/** The room numbers a bulk range creates: `prefix` + each number from `first` to `last`. */
export function rangeNumbers(prefix: string, first: number, last: number): string[] {
	const numbers: string[] = [];
	for (let n = first; n <= last; n++) numbers.push(`${prefix}${n}`);
	return numbers;
}
