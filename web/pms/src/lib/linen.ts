import { graphql } from './api/gql';
import type { LaundryBatchesQuery, LinenMovementsQuery, LinenQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import type { components } from './api/openapi';

/** The Linen page's items, locations, par table and stock, which change together under `linen:<property>`. */
export const LinenDocument = graphql(`
	query Linen($propertyId: UUID!) {
		linenItems(propertyId: $propertyId) {
			id
			code
			name
			active
			version
		}
		linenLocations(propertyId: $propertyId) {
			id
			name
			kind
			sectionId
			active
			version
		}
		linenPar(propertyId: $propertyId) {
			roomTypeId
			itemId
			quantity
		}
		linenStock(propertyId: $propertyId) {
			locationId
			itemId
			quantity
			par
			low
		}
	}
`);

/** The Batches tab: every batch, newest first, with what is still out. */
export const LaundryBatchesDocument = graphql(`
	query LaundryBatches($propertyId: UUID!) {
		laundryBatches(propertyId: $propertyId) {
			id
			fromLocationId
			toLocationId
			status
			note
			sentAt
			receivedAt
			lines {
				itemId
				sent
				returned
				writtenOff
				outstanding
			}
			outstanding
			settled
			version
		}
	}
`);

/** The Movements tab: the newest movements, from or to one location when given. */
export const LinenMovementsDocument = graphql(`
	query LinenMovements($propertyId: UUID!, $first: Int!, $locationId: UUID) {
		linenMovements(propertyId: $propertyId, first: $first, locationId: $locationId) {
			id
			itemId
			kind
			quantity
			fromLocationId
			toLocationId
			batchId
			reason
			businessDate
			movedAt
		}
	}
`);

export type LinenItem = LinenQuery['linenItems'][number];
export type LinenLocation = LinenQuery['linenLocations'][number];
export type LinenPar = LinenQuery['linenPar'][number];
export type LinenStock = LinenQuery['linenStock'][number];
export type LaundryBatch = LaundryBatchesQuery['laundryBatches'][number];
export type BatchLine = LaundryBatch['lines'][number];
export type LinenMovement = LinenMovementsQuery['linenMovements'][number];
export type LocationKind = LinenLocation['kind'];
/** A movement recorded by hand; `send` and `return` go with a batch. */
export type MoveKind = Extract<
	components['schemas']['MovementKind'],
	'receive' | 'transfer' | 'write_off'
>;

/** What `BatchReceiveDialog` counts: a batch coming back, or a received batch's items found or written off. */
export type BatchCountMode = 'receive' | 'return' | 'write_off';

/** The movements the Movements tab lists, the most the server returns at once. */
export const MOVEMENTS_SHOWN = 100;

/**
 * Every Linen query is under the server's `linen:<property>` event key, so the event refetches them all
 * (`events.ts`). `linenKey(propertyId)` alone is that prefix.
 */
export function linenKey(propertyId: string, ...rest: string[]) {
	return [`linen:${propertyId}`, ...rest] as const;
}

export async function fetchLinen(propertyId: string, signal?: AbortSignal) {
	return query(LinenDocument, { propertyId }, signal);
}

export async function fetchBatches(propertyId: string, signal?: AbortSignal) {
	return (await query(LaundryBatchesDocument, { propertyId }, signal)).laundryBatches;
}

export async function fetchMovements(
	propertyId: string,
	locationId: string | null,
	signal?: AbortSignal
) {
	return (
		await query(
			LinenMovementsDocument,
			{ propertyId, first: MOVEMENTS_SHOWN, locationId: locationId || null },
			signal
		)
	).linenMovements;
}

export interface StockCell {
	location: LinenLocation;
	quantity: number;
	par: number | null;
	low: boolean;
}

export interface StockRow {
	item: LinenItem;
	cells: StockCell[];
}

/**
 * The Stock tab's grid: one row per active item, by code, and one column per active location, by name. A cell the
 * server left out (an item or location switched on since) holds nothing, with no par.
 */
export function stockGrid(
	items: readonly LinenItem[],
	locations: readonly LinenLocation[],
	stock: readonly LinenStock[]
): { columns: LinenLocation[]; rows: StockRow[] } {
	const byCell = new Map(stock.map((cell) => [`${cell.locationId}|${cell.itemId}`, cell]));
	const columns = locations
		.filter((location) => location.active)
		.sort((a, b) => a.name.localeCompare(b.name));
	const rows = items
		.filter((item) => item.active)
		.sort((a, b) => a.code.localeCompare(b.code))
		.map((item) => ({
			item,
			cells: columns.map((location) => {
				const cell = byCell.get(`${location.id}|${item.id}`);
				return {
					location,
					quantity: cell?.quantity ?? 0,
					par: cell?.par ?? null,
					low: cell?.low ?? false
				};
			})
		}));
	return { columns, rows };
}

/** A stock cell in words: `12 of 20` with a par, else `12`. */
export function cellText(cell: Pick<StockCell, 'quantity' | 'par'>): string {
	return cell.par == null ? String(cell.quantity) : `${cell.quantity} of ${cell.par}`;
}

const CLEAN: readonly LocationKind[] = ['STORE', 'PANTRY'];
const OUT: readonly LocationKind[] = ['LAUNDRY', 'VENDOR'];

/**
 * The active locations a movement of `kind` may take linen from and to, as the server allows: a receive comes in to
 * a store or pantry, a transfer moves between them, and a write-off loses linen from anywhere. An empty side has no
 * location.
 */
export function moveLocations(
	kind: MoveKind,
	locations: readonly LinenLocation[]
): { from: LinenLocation[]; to: LinenLocation[] } {
	const active = locations.filter((location) => location.active);
	const clean = active.filter((location) => CLEAN.includes(location.kind));
	switch (kind) {
		case 'receive':
			return { from: [], to: clean };
		case 'transfer':
			return { from: clean, to: clean };
		case 'write_off':
			return { from: active, to: [] };
	}
}

/** Where a batch is sent from (a store or pantry) and to (a laundry or vendor). */
export function batchLocations(locations: readonly LinenLocation[]): {
	from: LinenLocation[];
	to: LinenLocation[];
} {
	const active = locations.filter((location) => location.active);
	return {
		from: active.filter((location) => CLEAN.includes(location.kind)),
		to: active.filter((location) => OUT.includes(location.kind))
	};
}

export interface CountLine {
	itemId: string;
	outstanding: number;
	counted: number;
	/** The count less what was still out: below zero is a shortfall. */
	difference: number;
}

/**
 * Each batch line still out, with what was counted (`counts` by item; an item not counted is 0) and the difference
 * from what was out.
 */
export function countDifferences(
	lines: readonly Pick<BatchLine, 'itemId' | 'outstanding'>[],
	counts: Readonly<Record<string, number>>
): CountLine[] {
	return lines
		.filter((line) => line.outstanding > 0)
		.map((line) => {
			const counted = counts[line.itemId] ?? 0;
			return {
				itemId: line.itemId,
				outstanding: line.outstanding,
				counted,
				difference: counted - line.outstanding
			};
		});
}

/** The counts a request sends, `{item_id, quantity}` for every item counted above zero. */
export function countItems(
	counts: Readonly<Record<string, number>>
): { item_id: string; quantity: number }[] {
	return Object.entries(counts)
		.filter(([, quantity]) => quantity > 0)
		.map(([item_id, quantity]) => ({ item_id, quantity }));
}

/** A batch's state in words. */
export function batchStatusText(batch: Pick<LaundryBatch, 'status' | 'outstanding'>): string {
	if (batch.status === 'SENT') return 'Sent';
	return batch.outstanding > 0 ? `Received, ${batch.outstanding} outstanding` : 'Settled';
}

const KIND_LABELS: Record<LocationKind, string> = {
	STORE: 'Store',
	PANTRY: 'Pantry',
	LAUNDRY: 'Laundry',
	VENDOR: 'Vendor'
};

export function locationKindLabel(kind: LocationKind): string {
	return KIND_LABELS[kind];
}

const MOVEMENT_LABELS: Record<LinenMovement['kind'], string> = {
	RECEIVE: 'Received',
	TRANSFER: 'Transfer',
	SEND: 'Sent',
	RETURN: 'Returned',
	WRITE_OFF: 'Write-off'
};

export function movementKindLabel(kind: LinenMovement['kind']): string {
	return MOVEMENT_LABELS[kind];
}
