import { graphql } from './api/gql';
import type { HousekeepingBoardQuery, MyRoomsQuery, RoomIssuesQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { idempotencyKey, ifMatch, rest, unwrap } from './api/rest';

export const HousekeepingBoardDocument = graphql(`
	query HousekeepingBoard($propertyId: UUID!) {
		housekeepingBoard(propertyId: $propertyId) {
			roomId
			number
			floor
			sectionId
			roomTypeId
			condition
			conditionVersion
			occupancy
			assignee {
				userId
				displayName
			}
			blockKind
		}
	}
`);

export const MyRoomsDocument = graphql(`
	query MyRooms($propertyId: UUID!) {
		myRooms(propertyId: $propertyId) {
			roomId
			number
			roomTypeId
			condition
			conditionVersion
			occupancy
			openIssues
			blockKind
		}
	}
`);

export const HousekeepersDocument = graphql(`
	query Housekeepers($propertyId: UUID!) {
		housekeepers(propertyId: $propertyId) {
			userId
			displayName
		}
	}
`);

export const RoomIssuesDocument = graphql(`
	query RoomIssues($propertyId: UUID!, $status: IssueStatus) {
		roomIssues(propertyId: $propertyId, status: $status) {
			id
			roomId
			severity
			description
			status
			reportedAt
			version
		}
	}
`);

export type BoardRoom = HousekeepingBoardQuery['housekeepingBoard'][number];
export type MyRoom = MyRoomsQuery['myRooms'][number];
export type RoomIssue = RoomIssuesQuery['roomIssues'][number];
export type Condition = BoardRoom['condition'];
export type Occupancy = BoardRoom['occupancy'];
export type Severity = RoomIssue['severity'];

/** Conditions and assignments (and check-in and check-out) change under `hk:<property>`. */
export function hkKey(propertyId: string) {
	return [`hk:${propertyId}`] as const;
}

/** The housekeeper app's list, under the property's `hk:` key so its events refresh it. */
export function myRoomsKey(propertyId: string) {
	return [`hk:${propertyId}`, 'mine'] as const;
}

/** Room issues change under `issues:<property>`. */
export function issuesKey(propertyId: string) {
	return [`issues:${propertyId}`] as const;
}

export async function fetchBoard(propertyId: string, signal?: AbortSignal) {
	return (await query(HousekeepingBoardDocument, { propertyId }, signal)).housekeepingBoard;
}

export async function fetchMyRooms(propertyId: string, signal?: AbortSignal) {
	return (await query(MyRoomsDocument, { propertyId }, signal)).myRooms;
}

export async function fetchHousekeepers(propertyId: string, signal?: AbortSignal) {
	return (await query(HousekeepersDocument, { propertyId }, signal)).housekeepers;
}

export async function fetchOpenIssues(propertyId: string, signal?: AbortSignal) {
	return (await query(RoomIssuesDocument, { propertyId, status: 'OPEN' }, signal)).roomIssues;
}

export const CONDITIONS: readonly Condition[] = ['DIRTY', 'CLEANING', 'CLEAN', 'INSPECTED'];
export const OCCUPANCIES: readonly Occupancy[] = [
	'VACANT',
	'STAY_OVER',
	'DUE_OUT',
	'DUE_IN',
	'TURNOVER'
];

const CONDITION_LABELS: Record<Condition, string> = {
	DIRTY: 'Dirty',
	CLEANING: 'Cleaning',
	CLEAN: 'Clean',
	INSPECTED: 'Inspected'
};

const OCCUPANCY_LABELS: Record<Occupancy, string> = {
	VACANT: 'Vacant',
	STAY_OVER: 'Stay-over',
	DUE_OUT: 'Due out',
	DUE_IN: 'Due in',
	TURNOVER: 'Turnover'
};

/** Whether a check-in refusal is the room's condition ("room 101 is Dirty"), which a supervisor may override. */
export function isConditionRefusal(status: number, detail: string | undefined): boolean {
	return status === 409 && /^room .+ is (Dirty|Cleaning)$/.test(detail ?? '');
}

export const conditionLabel = (condition: Condition) => CONDITION_LABELS[condition];
export const occupancyLabel = (occupancy: Occupancy) => OCCUPANCY_LABELS[occupancy];

/**
 * The conditions a user may move a room to from `from`, mirroring the server: a supervisor may set any
 * other value; a housekeeper only advances `dirty → cleaning → clean`. Without `from`, a supervisor's
 * choices are all four.
 */
export function nextConditions(role: 'supervisor' | 'housekeeper', from?: Condition): Condition[] {
	if (role === 'supervisor') return CONDITIONS.filter((condition) => condition !== from);
	if (from === 'DIRTY') return ['CLEANING'];
	if (from === 'CLEANING') return ['CLEAN'];
	return [];
}

/** Initials of a name, at most two letters: "Kumari Silva" is "KS". */
export function initials(name: string): string {
	return name
		.split(/\s+/)
		.filter(Boolean)
		.slice(0, 2)
		.map((word) => word[0].toUpperCase())
		.join('');
}

export interface BoardFilters {
	floor: string;
	section: string;
	/** A housekeeper's user id, or `none` for unassigned rooms. */
	housekeeper: string;
	condition: Condition | '';
	occupancy: Occupancy | '';
}

export const NO_FILTERS: BoardFilters = {
	floor: '',
	section: '',
	housekeeper: '',
	condition: '',
	occupancy: ''
};

/** The floor filter's value for rooms without a floor. */
export const NO_FLOOR = 'none';

/** The filters in the URL: `floor`, `section`, `hk`, `condition` and `occupancy`, each omitted when unset. */
export function filtersFromSearchParams(p: URLSearchParams): BoardFilters {
	const condition = (p.get('condition') ?? '').toUpperCase();
	const occupancy = (p.get('occupancy') ?? '').toUpperCase();
	return {
		floor: p.get('floor') ?? '',
		section: p.get('section') ?? '',
		housekeeper: p.get('hk') ?? '',
		condition: CONDITIONS.find((value) => value === condition) ?? '',
		occupancy: OCCUPANCIES.find((value) => value === occupancy) ?? ''
	};
}

export function filtersToSearchParams(filters: BoardFilters): URLSearchParams {
	const p = new URLSearchParams();
	if (filters.floor) p.set('floor', filters.floor);
	if (filters.section) p.set('section', filters.section);
	if (filters.housekeeper) p.set('hk', filters.housekeeper);
	if (filters.condition) p.set('condition', filters.condition.toLowerCase());
	if (filters.occupancy) p.set('occupancy', filters.occupancy.toLowerCase());
	return p;
}

/** The rooms that match every filter that is set. */
export function filterRooms(rooms: readonly BoardRoom[], filters: BoardFilters): BoardRoom[] {
	return rooms.filter(
		(room) =>
			(!filters.floor || (room.floor ?? NO_FLOOR) === filters.floor) &&
			(!filters.section || room.sectionId === filters.section) &&
			(!filters.housekeeper ||
				(filters.housekeeper === 'none'
					? !room.assignee
					: room.assignee?.userId === filters.housekeeper)) &&
			(!filters.condition || room.condition === filters.condition) &&
			(!filters.occupancy || room.occupancy === filters.occupancy)
	);
}

/** Open issues per room, for the badge. */
export function issueCounts(issues: readonly RoomIssue[]): Map<string, number> {
	const counts = new Map<string, number>();
	for (const issue of issues) counts.set(issue.roomId, (counts.get(issue.roomId) ?? 0) + 1);
	return counts;
}

/** The ids from `from` to `to` (either order) in `order`, for a shift-click; just `to` if `from` is absent. */
export function rangeBetween(order: readonly string[], from: string | undefined, to: string) {
	const a = from === undefined ? -1 : order.indexOf(from);
	const b = order.indexOf(to);
	if (b < 0) return [];
	if (a < 0) return [to];
	return order.slice(Math.min(a, b), Math.max(a, b) + 1);
}

/**
 * The index of the tile a navigation key moves to in a board of `count` tiles laid out `columns` to a row, or
 * `null` if the key does not navigate. Arrows move by tile and row, Home and End go to the ends of the row
 * (Ctrl to the ends of the board), PageUp and PageDown move `pageRows` rows. The last row may be short.
 */
export function moveTile(
	index: number,
	key: string,
	ctrl: boolean,
	columns: number,
	count: number,
	pageRows: number
): number | null {
	const clamp = (value: number) => Math.max(0, Math.min(count - 1, value));
	switch (key) {
		case 'ArrowLeft':
			return clamp(index - 1);
		case 'ArrowRight':
			return clamp(index + 1);
		case 'ArrowUp':
			return clamp(index - columns);
		case 'ArrowDown':
			return clamp(index + columns);
		case 'PageUp':
			return clamp(index - pageRows * columns);
		case 'PageDown':
			return clamp(index + pageRows * columns);
		case 'Home':
			return ctrl ? 0 : index - (index % columns);
		case 'End':
			return ctrl ? count - 1 : clamp(index - (index % columns) + columns - 1);
		default:
			return null;
	}
}

/** The button for moving a room to `condition`: a housekeeper's two steps, or a supervisor's "Set …". */
export function stepLabel(condition: Condition): string {
	if (condition === 'CLEANING') return 'Start cleaning';
	if (condition === 'CLEAN') return 'Done';
	return `Set ${conditionLabel(condition).toLowerCase()}`;
}

/** Sets one room's condition; `version` is the condition version it was read at, sent as If-Match. */
export async function setCondition(
	propertyId: string,
	roomId: string,
	condition: Condition,
	version: number
) {
	return unwrap(
		await rest.PUT('/api/v1/properties/{property}/rooms/{room}/condition', {
			params: { path: { property: propertyId, room: roomId }, header: ifMatch(version) },
			body: { condition: condition.toLowerCase() as Lowercase<Condition> }
		})
	);
}

/** The most rooms one bulk condition or assignment change may name; the server answers 422 beyond it. */
export const MAX_BULK_ROOMS = 500;

/** The longest issue description, as on the server. */
export const MAX_ISSUE_DESCRIPTION = 2000;

/** Sets the condition of every room in `roomIds` (at most {@link MAX_BULK_ROOMS}); the key makes a retry safe. */
export async function setConditions(
	propertyId: string,
	roomIds: string[],
	condition: Condition,
	key = idempotencyKey()
) {
	return unwrap(
		await rest.POST('/api/v1/properties/{property}/rooms/conditions', {
			params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
			body: { room_ids: roomIds, condition: condition.toLowerCase() as Lowercase<Condition> }
		})
	);
}

/** Assigns every room to `userId` for the business date, or unassigns them with `null`. */
export async function assignRooms(propertyId: string, roomIds: string[], userId: string | null) {
	unwrap(
		await rest.PUT('/api/v1/properties/{property}/hk-assignments', {
			params: { path: { property: propertyId } },
			body: { assignments: roomIds.map((room_id) => ({ room_id, user_id: userId })) }
		})
	);
}

export async function copyYesterday(propertyId: string, key = idempotencyKey()): Promise<number> {
	return unwrap(
		await rest.POST('/api/v1/properties/{property}/hk-assignments/copy-yesterday', {
			params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } }
		})
	).copied;
}

export async function reportIssue(
	propertyId: string,
	roomId: string,
	severity: Severity,
	description: string,
	key = idempotencyKey()
) {
	return unwrap(
		await rest.POST('/api/v1/properties/{property}/room-issues', {
			params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
			body: {
				room_id: roomId,
				severity: severity.toLowerCase() as Lowercase<Severity>,
				description
			}
		})
	);
}

/** Resolves an open issue; `version` is sent as If-Match. */
export async function resolveIssue(propertyId: string, issueId: string, version: number) {
	unwrap(
		await rest.PATCH('/api/v1/properties/{property}/room-issues/{issue}', {
			params: { path: { property: propertyId, issue: issueId }, header: ifMatch(version) },
			body: { status: 'resolved' }
		})
	);
}
