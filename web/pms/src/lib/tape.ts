import type { QueryClient } from '@tanstack/svelte-query';
import { graphql } from './api/gql';
import type { TapeWindowQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { addDays } from './inventory';
import type { Room, RoomType } from './rooms';

export const TILE_DAYS = 14;
export const TILE_EPOCH = '2020-01-06'; // a Monday
export const PAGE_SIZE = 10;
export const LRU_TILES = 12;
export type Span = 7 | 14 | 30;
/** The chart opens this many days before the business date, so yesterday's departures show. */
const OPENING_DAYS_BEFORE = 2;

const SPANS: readonly Span[] = [7, 14, 30];
const DEFAULT_SPAN: Span = 14;
const MAX_SUGGESTIONS = 8;
const MS_PER_DAY = 86_400_000;

/** Whole days since 1970-01-01 for a `YYYY-MM-DD` date, on UTC midnight so no time zone can shift it. */
function dayNumber(date: string): number {
	return Math.round(Date.parse(`${date}T00:00:00Z`) / MS_PER_DAY);
}

function isIsoDate(value: string | null): value is string {
	return (
		value !== null &&
		/^\d{4}-\d{2}-\d{2}$/.test(value) &&
		!Number.isNaN(Date.parse(`${value}T00:00:00Z`))
	);
}

const EPOCH_DAY = dayNumber(TILE_EPOCH);

/** The start of the 14-day tile containing `date`; tiles are `TILE_EPOCH + 14·k`, also for k < 0. */
export function tileStartFor(date: string): string {
	const k = Math.floor((dayNumber(date) - EPOCH_DAY) / TILE_DAYS);
	return addDays(TILE_EPOCH, k * TILE_DAYS);
}

/**
 * The tiles to fetch for a viewport of `days` days from `viewStart`: those it intersects, plus `overscan`
 * (0 or 1) more beyond the edge `direction` scrolls towards. Direction 0 adds none.
 */
export function tilesFor(
	viewStart: string,
	days: number,
	overscan: 0 | 1,
	direction: -1 | 0 | 1
): string[] {
	let first = tileStartFor(viewStart);
	let last = tileStartFor(addDays(viewStart, Math.max(days, 1) - 1));
	if (direction === -1) first = addDays(first, -TILE_DAYS * overscan);
	if (direction === 1) last = addDays(last, TILE_DAYS * overscan);
	const tiles: string[] = [];
	for (let tile = first; tile <= last; tile = addDays(tile, TILE_DAYS)) tiles.push(tile);
	return tiles;
}

/** The day after the tile's last day (exclusive end). */
export function tileEnd(tileStart: string): string {
	return addDays(tileStart, TILE_DAYS);
}

/** The months (`YYYY-MM`) a tile touches: one, or two when it straddles a month end. */
export function monthsOfTile(tileStart: string): string[] {
	const first = tileStart.slice(0, 7);
	const last = addDays(tileStart, TILE_DAYS - 1).slice(0, 7);
	return first === last ? [first] : [first, last];
}

/** The distinct tiles of `cached` that touch `month` (`YYYY-MM`). */
export function tilesInMonth(month: string, cached: readonly string[]): string[] {
	return [...new Set(cached)].filter((tile) => monthsOfTile(tile).includes(month));
}

export type Chip =
	| { kind: 'type'; roomTypeId: string; label: string }
	| { kind: 'room'; roomId: string; label: string }
	| { kind: 'range'; from: string; to: string; label: string };

/** An active room as the rail and the picker see it. */
export interface RailRoom {
	id: string;
	number: string;
	roomTypeId: string;
	typeCode: string;
}

const NATURAL_CHUNK = /(\d+)|(\D+)/g;

/** Compares numbers naturally, digit runs by value: `99` < `101` < `1010`, `A2` < `A10`. */
export function naturalCompare(a: string, b: string): number {
	const left = a.match(NATURAL_CHUNK) ?? [];
	const right = b.match(NATURAL_CHUNK) ?? [];
	for (let i = 0; i < Math.min(left.length, right.length); i++) {
		const x = left[i];
		const y = right[i];
		const bothDigits = /^\d/.test(x) && /^\d/.test(y);
		if (bothDigits) {
			const xs = x.replace(/^0+(?=\d)/, '');
			const ys = y.replace(/^0+(?=\d)/, '');
			if (xs.length !== ys.length) return xs.length - ys.length;
			if (xs !== ys) return xs < ys ? -1 : 1;
		} else {
			const xl = x.toLowerCase();
			const yl = y.toLowerCase();
			if (xl !== yl) return xl < yl ? -1 : 1;
		}
	}
	if (left.length !== right.length) return left.length - right.length;
	return a === b ? 0 : a < b ? -1 : 1;
}

function rangeChip(a: string, b: string): Chip {
	const [from, to] = naturalCompare(a, b) <= 0 ? [a, b] : [b, a];
	return { kind: 'range', from, to, label: `${from}-${to}` };
}

const RANGE_TEXT = /^([^\s\-–,]+)\s*[-–]\s*([^\s\-–,]+)$/;

/** Chips for what the user has typed: matching types, then a range if it is one, then rooms; at most 8. */
export function suggest(text: string, rooms: RailRoom[], types: RoomType[]): Chip[] {
	const needle = text.trim().toLowerCase();
	if (!needle) return [];
	const chips: Chip[] = [];
	for (const type of types) {
		if (type.code.toLowerCase().startsWith(needle) || type.name.toLowerCase().includes(needle)) {
			chips.push({ kind: 'type', roomTypeId: type.id, label: `${type.code} ${type.name}` });
		}
	}
	const range = RANGE_TEXT.exec(text.trim());
	if (range) chips.push(rangeChip(range[1], range[2]));
	for (const room of rooms) {
		if (room.number.toLowerCase().startsWith(needle)) {
			chips.push({ kind: 'room', roomId: room.id, label: room.number });
		}
	}
	return chips.slice(0, MAX_SUGGESTIONS);
}

/** Active rooms in the rail's order: room type order, then room order, then number naturally. */
export function railRooms(rooms: Room[], types: RoomType[]): RailRoom[] {
	const typeById = new Map(types.map((type) => [type.id, type]));
	return rooms
		.filter((room) => room.active && typeById.has(room.roomTypeId))
		.sort(
			(a, b) =>
				typeById.get(a.roomTypeId)!.sortOrder - typeById.get(b.roomTypeId)!.sortOrder ||
				a.sortOrder - b.sortOrder ||
				naturalCompare(a.number, b.number)
		)
		.map((room) => ({
			id: room.id,
			number: room.number,
			roomTypeId: room.roomTypeId,
			typeCode: typeById.get(room.roomTypeId)!.code
		}));
}

function chipMatches(chip: Chip, room: RailRoom): boolean {
	switch (chip.kind) {
		case 'type':
			return room.roomTypeId === chip.roomTypeId;
		case 'room':
			return room.id === chip.roomId;
		case 'range':
			return (
				naturalCompare(room.number, chip.from) >= 0 && naturalCompare(room.number, chip.to) <= 0
			);
	}
}

/** The rail's rooms any chip selects, in rail order and without duplicates; no chips selects every room. */
export function selectRooms(rail: RailRoom[], chips: readonly Chip[]): RailRoom[] {
	if (chips.length === 0) return rail;
	return rail.filter((room) => chips.some((chip) => chipMatches(chip, room)));
}

/** One page (1-based) of `PAGE_SIZE` rooms. A page beyond the last, or below the first, is clamped. */
export function pageOf(
	selected: RailRoom[],
	page: number
): { rooms: RailRoom[]; page: number; pages: number } {
	const pages = Math.max(1, Math.ceil(selected.length / PAGE_SIZE));
	const clamped = Math.min(Math.max(Number.isFinite(page) ? Math.trunc(page) : 1, 1), pages);
	return {
		rooms: selected.slice((clamped - 1) * PAGE_SIZE, clamped * PAGE_SIZE),
		page: clamped,
		pages
	};
}

/** The chart's view; `page` is 1-based and `start` is the first visible day. */
export interface TapeView {
	chips: Chip[];
	page: number;
	span: Span;
	start: string;
}

function chipFromParam(token: string, rooms: RailRoom[], types: RoomType[]): Chip | null {
	const value = token.slice(2);
	if (token.startsWith('t:')) {
		const type = types.find((candidate) => candidate.id === value);
		return type ? { kind: 'type', roomTypeId: type.id, label: type.code } : null;
	}
	if (token.startsWith('r:')) {
		const room = rooms.find((candidate) => candidate.id === value);
		return room ? { kind: 'room', roomId: room.id, label: room.number } : null;
	}
	if (token.startsWith('n:')) {
		const bounds = /^([^-]+)-([^-]+)$/.exec(value);
		return bounds ? rangeChip(bounds[1], bounds[2]) : null;
	}
	return null;
}

/** The view a URL describes; unknown ids and malformed values are dropped, and the page is clamped. */
export function viewFromSearchParams(
	p: URLSearchParams,
	businessDate: string,
	rooms: RailRoom[],
	types: RoomType[]
): TapeView {
	const chips = (p.get('pick') ?? '')
		.split(',')
		.map((token) => chipFromParam(token, rooms, types))
		.filter((chip): chip is Chip => chip !== null);
	const span = SPANS.find((candidate) => String(candidate) === p.get('span')) ?? DEFAULT_SPAN;
	const startParam = p.get('start');
	const start = isIsoDate(startParam) ? startParam : businessDate;
	const requested = Number.parseInt(p.get('page') ?? '1', 10);
	const { page } = pageOf(selectRooms(rooms, chips), requested);
	return { chips, page, span, start };
}

/** The URL form of a view: chips as `t:<id>`, `r:<id>`, `n:<from>-<to>` under `pick`; page 1 is omitted. */
export function viewToSearchParams(v: TapeView): URLSearchParams {
	const p = new URLSearchParams();
	const pick = v.chips.map((chip) => {
		switch (chip.kind) {
			case 'type':
				return `t:${chip.roomTypeId}`;
			case 'room':
				return `r:${chip.roomId}`;
			case 'range':
				return `n:${chip.from}-${chip.to}`;
		}
	});
	if (pick.length > 0) p.set('pick', pick.join(','));
	if (v.page > 1) p.set('page', String(v.page));
	p.set('span', String(v.span));
	p.set('start', v.start);
	return p;
}

/** The page's rooms as one cache-key part. */
export function pageKey(roomIds: readonly string[]): string {
	return roomIds.join(',');
}

/** Query key of one tile of one room page; `key` is `pageKey`. */
export function tapeKey(propertyId: string, tileStart: string, key: string): readonly unknown[] {
	return ['tape', propertyId, tileStart, key] as const;
}

/** The cached tiles to refetch for a server event key `tape:<property>:<YYYY-MM>`; others give none. */
export function tapeEventTiles(
	eventKey: string,
	propertyId: string,
	cachedTiles: readonly string[]
): string[] {
	const prefix = `tape:${propertyId}:`;
	if (!eventKey.startsWith(prefix)) return [];
	return tilesInMonth(eventKey.slice(prefix.length), cachedTiles);
}

export interface TapeStay {
	id: string;
	reservationId: string;
	roomId: string;
	roomTypeId: string;
	start: string;
	end: string;
	status: string;
	guestName: string;
	accountName: string | null;
	version: number;
}

export interface TapeBlock {
	id: string;
	roomId: string;
	start: string;
	end: string;
	reason: string;
}

/** One fetched tile of one room page. */
export interface TapeTile {
	stays: TapeStay[];
	blocks: TapeBlock[];
}

export type TapeBar = ({ kind: 'stay' } & TapeStay) | ({ kind: 'block' } & TapeBlock);

/** Every stay and block of the tiles once: a stay in two tiles is one bar (the higher version wins). */
export function barsFor(tiles: readonly TapeTile[]): TapeBar[] {
	const stays = new Map<string, TapeStay>();
	const blocks = new Map<string, TapeBlock>();
	for (const tile of tiles) {
		for (const stay of tile.stays) {
			const seen = stays.get(stay.id);
			if (!seen || stay.version > seen.version) stays.set(stay.id, stay);
		}
		for (const block of tile.blocks) if (!blocks.has(block.id)) blocks.set(block.id, block);
	}
	return [
		...[...stays.values()].map((stay): TapeBar => ({ kind: 'stay', ...stay })),
		...[...blocks.values()].map((block): TapeBar => ({ kind: 'block', ...block }))
	];
}

/** Pixels from the chart's left edge to the day `start` (may be negative when it starts before the view). */
export function barX(start: string, viewStart: string, dayWidth: number): number {
	return (dayNumber(start) - dayNumber(viewStart)) * dayWidth;
}

/** Pixels from `start` to the exclusive `end`. */
export function barWidth(start: string, end: string, dayWidth: number): number {
	return Math.max(dayNumber(end) - dayNumber(start), 0) * dayWidth;
}

export const TapeWindowDocument = graphql(`
	query TapeWindow($property: UUID!, $rooms: [UUID!]!, $from: Date!, $to: Date!) {
		tapeWindow(propertyId: $property, roomIds: $rooms, from: $from, to: $to) {
			stays {
				id
				reservationId
				roomId
				roomTypeId
				start
				end
				status
				guestName
				accountName
				version
			}
			blocks {
				id
				roomId
				start
				end
				reason
			}
		}
	}
`);

export type TapeWindow = TapeWindowQuery['tapeWindow'];

/** One tile (14 days from `tileStart`) of the stays and blocks of `roomIds`, at most ten rooms. */
export async function fetchTapeTile(
	propertyId: string,
	tileStart: string,
	roomIds: readonly string[],
	signal?: AbortSignal
): Promise<TapeTile> {
	const { tapeWindow } = await query(
		TapeWindowDocument,
		{ property: propertyId, rooms: [...roomIds], from: tileStart, to: tileEnd(tileStart) },
		signal
	);
	return tapeWindow;
}

/** The first visible day of the chart's opening view. */
export function openingStart(businessDate: string): string {
	return addDays(businessDate, -OPENING_DAYS_BEFORE);
}

/** A cached tape query as the LRU sees it: when it last got data, and whether it is observed or fetching. */
export interface TapeEntry {
	key: string;
	updatedAt: number;
	busy: boolean;
}

/** The keys to remove so at most `limit` entries remain: the least recently updated, never a busy one. */
export function lruVictims(entries: readonly TapeEntry[], limit: number): string[] {
	const surplus = entries.length - limit;
	if (surplus <= 0) return [];
	return entries
		.filter((entry) => !entry.busy)
		.sort((a, b) => a.updatedAt - b.updatedAt)
		.slice(0, surplus)
		.map((entry) => entry.key);
}

/** Removes the oldest cached tape tiles beyond `LRU_TILES`, keeping any that is observed or fetching. */
export function pruneTape(client: QueryClient): void {
	const cached = client.getQueryCache().findAll({ queryKey: ['tape'] });
	const victims = new Set(
		lruVictims(
			cached.map((query) => ({
				key: query.queryHash,
				updatedAt: query.state.dataUpdatedAt,
				busy: query.getObserversCount() > 0 || query.state.fetchStatus === 'fetching'
			})),
			LRU_TILES
		)
	);
	if (victims.size > 0) {
		client.removeQueries({
			queryKey: ['tape'],
			predicate: (query) => victims.has(query.queryHash)
		});
	}
}

/** Warms the cache with `tiles` of the room page `roomIds`, as the chart would fetch them. */
export async function prefetchTapeTiles(
	client: QueryClient,
	propertyId: string,
	tiles: readonly string[],
	roomIds: readonly string[]
): Promise<void> {
	const key = pageKey(roomIds);
	await Promise.all(
		tiles.map((tile) =>
			client.prefetchQuery({
				queryKey: tapeKey(propertyId, tile, key),
				queryFn: ({ signal }) => fetchTapeTile(propertyId, tile, roomIds, signal),
				staleTime: Infinity
			})
		)
	);
	pruneTape(client);
}
