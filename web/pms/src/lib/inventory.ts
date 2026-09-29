import { graphql } from './api/gql';
import type { InventoryQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import type { components } from './api/openapi';
import { ApiError } from './api/problem';

export const InventoryDocument = graphql(`
	query Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {
		inventory(propertyId: $propertyId, from: $from, to: $to) {
			date
			roomTypeId
			physical
			sold
			outOfOrder
			available
			sellable
		}
		blocks(propertyId: $propertyId, from: $from, to: $to) {
			id
			roomId
			from
			to
			kind
			reasonId
			note
			version
		}
	}
`);

export type InventoryDay = InventoryQuery['inventory'][number];
export type Block = InventoryQuery['blocks'][number];

/** Query key shared with the server's `inventory:<property>:<yyyy-mm>` event. */
export function inventoryKey(propertyId: string, month: string) {
	return [`inventory:${propertyId}:${month}`] as const;
}

/** A month's counts and blocks. `month` is `YYYY-MM`. */
export async function fetchMonth(propertyId: string, month: string, signal?: AbortSignal) {
	const days = monthDays(month);
	return query(
		InventoryDocument,
		{ propertyId, from: days[0], to: addDays(days[days.length - 1], 1) },
		signal
	);
}

/** `YYYY-MM-DD` plus `days`, in calendar days (no time zones involved). */
export function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

/** The month (`YYYY-MM`) a date is in. */
export function monthOf(date: string): string {
	return date.slice(0, 7);
}

/** `YYYY-MM` moved by `delta` months. */
export function shiftMonth(month: string, delta: number): string {
	const [year, index] = month.split('-').map(Number);
	const moved = new Date(Date.UTC(year, index - 1 + delta, 1));
	return moved.toISOString().slice(0, 7);
}

/** Every day of `month` as `YYYY-MM-DD`. */
export function monthDays(month: string): string[] {
	const days: string[] = [];
	for (let day = `${month}-01`; monthOf(day) === month; day = addDays(day, 1)) days.push(day);
	return days;
}

/** Looks up a room type's counts on a day. */
export function indexInventory(rows: InventoryDay[]) {
	const byKey = new Map(rows.map((row) => [`${row.roomTypeId}|${row.date}`, row]));
	return {
		get: (roomTypeId: string, date: string) => byKey.get(`${roomTypeId}|${date}`)
	};
}

/** Blocks covering `date` (from its first day up to, not including, `to`), optionally only for `rooms`. */
export function blocksOn(blocks: Block[], date: string, rooms?: Set<string>): Block[] {
	return blocks.filter(
		(block) => block.from <= date && date < block.to && (!rooms || rooms.has(block.roomId))
	);
}

type BlockConflict = Pick<components['schemas']['Block'], 'room_id' | 'from' | 'to' | 'kind'>;

/** One sentence per block listed in a 409's `conflicts`; empty for any other error. */
export function conflictMessages(error: unknown, roomNumber: (roomId: string) => string): string[] {
	if (!(error instanceof ApiError) || !Array.isArray(error.problem.conflicts)) return [];
	return (error.problem.conflicts as BlockConflict[]).map(
		(block) =>
			`Room ${roomNumber(block.room_id)} is already blocked from ${block.from} until ${block.to} (${block.kind.replaceAll('_', ' ')}).`
	);
}
