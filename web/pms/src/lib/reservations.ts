import { graphql } from './api/gql';
import type {
	AvailabilityQuery,
	FreeRoomsQuery,
	GuestsQuery,
	MealPlan,
	Residency,
	ReservationListQuery,
	ReservationQuery,
	ReservationSortField,
	RoomStatus,
	SortDirection,
	Source
} from './api/gql/graphql';
import { query } from './api/graphql';
import { formatMoney } from './rates';

/** The new-reservation screen's offers query: every active room type, free counts and priced offers. */
export const AvailabilityDocument = graphql(`
	query Availability(
		$propertyId: UUID!
		$checkIn: Date!
		$checkOut: Date!
		$adults: Int!
		$children: Int!
		$residency: Residency!
	) {
		availability(
			propertyId: $propertyId
			checkIn: $checkIn
			checkOut: $checkOut
			adults: $adults
			children: $children
			residency: $residency
		) {
			roomTypeId
			code
			name
			free
			offers {
				ratePlanId
				ratePlanCode
				mealPlan
				total
				currency
				restrictionsOk
				violations {
					kind
					message
				}
				nights {
					date
					room
					meal
				}
			}
		}
	}
`);

/**
 * The reservations table's query, exactly as validated against the server's GraphQL depth and complexity
 * limits (`crates/core-api/tests/reservation_reads.rs`'s `LIST`). One node per reservation room.
 */
export const ReservationsDocument = graphql(`
	query ReservationList(
		$p: UUID!
		$filter: ReservationFilter
		$sort: ReservationSort
		$first: Int
		$after: String
		$withCount: Boolean!
	) {
		reservations(propertyId: $p, filter: $filter, sort: $sort, first: $first, after: $after) {
			nodes {
				id
				reservationId
				confirmationNo
				guestName
				arrival
				departure
				nights
				roomTypeCode
				roomNumber
				status
				source
				total
				currency
				version
			}
			pageInfo {
				endCursor
				hasNextPage
			}
			totalCount @include(if: $withCount)
		}
	}
`);

/**
 * The reservation modal's query, exactly as validated against the server's GraphQL depth and complexity
 * limits (`crates/core-api/tests/reservation_reads.rs`'s `DETAIL`).
 */
export const ReservationDocument = graphql(`
	query Reservation($p: UUID!, $id: UUID!) {
		reservation(propertyId: $p, id: $id) {
			id
			confirmationNo
			status
			source
			notes
			createdAt
			version
			booker {
				id
				firstName
				lastName
				email
				phone
				country
				residency
				idDocType
				idDocMasked
				notes
				version
			}
			totals {
				currency
				amount
			}
			rooms {
				id
				version
				status
				checkIn
				checkOut
				adults
				children
				mealPlan
				total
				currency
				roomType {
					id
					code
					name
				}
				room {
					id
					number
				}
				ratePlan {
					id
					code
				}
				primaryGuest {
					id
					firstName
					lastName
					residency
					idDocType
					idDocMasked
				}
				nights {
					date
					room
					meal
				}
				cancellationTerms {
					rules {
						daysBeforeArrival
						penalty {
							kind
							value
						}
					}
					noShow {
						kind
						value
					}
				}
				cancellationPenalty
				cancelledAt
				recordedPenalty
			}
			history {
				action
				at
				actorName
				data
			}
		}
	}
`);

/** Guest search for the new-reservation screen and the assign picker's "book for a new guest" step. */
export const GuestsDocument = graphql(`
	query Guests($propertyId: UUID!, $search: String, $first: Int) {
		guests(propertyId: $propertyId, search: $search, first: $first) {
			id
			firstName
			lastName
			email
			phone
			country
			residency
			idDocType
			idDocMasked
			notes
			version
		}
	}
`);

/** The assign picker: active rooms of the booked type free for the stay. */
export const FreeRoomsDocument = graphql(`
	query FreeRooms($propertyId: UUID!, $roomTypeId: UUID!, $checkIn: Date!, $checkOut: Date!) {
		freeRooms(
			propertyId: $propertyId
			roomTypeId: $roomTypeId
			checkIn: $checkIn
			checkOut: $checkOut
		) {
			id
			number
			section
		}
	}
`);

export type RoomTypeAvailability = AvailabilityQuery['availability'][number];
export type Offer = RoomTypeAvailability['offers'][number];
export type ReservationRoomRow = ReservationListQuery['reservations']['nodes'][number];
export type ReservationDetail = ReservationQuery['reservation'];
export type ReservationRoom = ReservationDetail['rooms'][number];
export type Guest = GuestsQuery['guests'][number];
export type FreeRoom = FreeRoomsQuery['freeRooms'][number];

/** Which reservation rooms to list. Left out, a field does not filter; an empty `statuses`/`sources` matches
 * nothing (mirrors the server: "empty selections never mean everything"). */
export interface ReservationFilter {
	arrivalFrom?: string | null;
	/** Inclusive. */
	arrivalTo?: string | null;
	statuses?: RoomStatus[] | null;
	sources?: Source[] | null;
	/** The start of a confirmation number, in any case, or a guest's name, typos included. */
	text?: string | null;
}

export interface ReservationSort {
	field: ReservationSortField;
	direction: SortDirection;
}

export interface ReservationListParams {
	filter: ReservationFilter;
	sort: ReservationSort;
}

/** No sort given: the server's own default, and this module's. */
export const DEFAULT_SORT: ReservationSort = { field: 'ARRIVAL', direction: 'ASC' };

const EMPTY_FILTER: ReservationFilter = {};

export const DEFAULT_LIST_PARAMS: ReservationListParams = {
	filter: EMPTY_FILTER,
	sort: DEFAULT_SORT
};

/**
 * Query key shared with the server's `reservations:<property>` event. The event names only the bare
 * `reservations:<property>` string; `events.ts` invalidates with `client.invalidateQueries({ queryKey: [key] })`,
 * which is a *prefix* match (TanStack Query's default), not an exact-key match. So this key's first element
 * must be exactly `reservations:<property>` — everything after it (here, the filter/sort `params`) can vary
 * freely per screen and the event still invalidates every one of them.
 */
export function reservationsKey(propertyId: string, params?: ReservationListParams) {
	return [`reservations:${propertyId}`, params ?? null] as const;
}

/** Query key shared with the server's `reservation:<id>` event. */
export function reservationKey(id: string) {
	return [`reservation:${id}`] as const;
}

/** Query key for one availability lookup. Not named by any server event: a stay's offers are refetched by
 * asking again (new dates, new occupancy), never invalidated, so every argument that changes the answer is
 * part of the key. */
export function availabilityKey(
	propertyId: string,
	checkIn: string,
	checkOut: string,
	adults: number,
	children: number,
	residency: Residency
) {
	return ['availability', propertyId, checkIn, checkOut, adults, children, residency] as const;
}

/** Every active room type's free count and priced offers for a stay of `[checkIn, checkOut)`. */
export async function fetchAvailability(
	propertyId: string,
	checkIn: string,
	checkOut: string,
	adults: number,
	children: number,
	residency: Residency,
	signal?: AbortSignal
) {
	return (
		await query(
			AvailabilityDocument,
			{ propertyId, checkIn, checkOut, adults, children, residency },
			signal
		)
	).availability;
}

/** A page of the reservations table. Only the first page (no cursor) asks for `totalCount`: the server
 * counts every match only when it is selected, and later pages keep the first page's total. */
export async function fetchReservations(
	propertyId: string,
	params: ReservationListParams = DEFAULT_LIST_PARAMS,
	first?: number,
	after?: string,
	signal?: AbortSignal
) {
	return (
		await query(
			ReservationsDocument,
			{
				p: propertyId,
				filter: params.filter,
				sort: params.sort,
				first,
				after,
				withCount: after === undefined
			},
			signal
		)
	).reservations;
}

/** One reservation with its rooms and history. */
export async function fetchReservation(propertyId: string, id: string, signal?: AbortSignal) {
	return (await query(ReservationDocument, { p: propertyId, id }, signal)).reservation;
}

/** Guests whose name is like `search`, typos included, or whose email/phone matches it exactly. */
export async function fetchGuests(
	propertyId: string,
	search?: string,
	first?: number,
	signal?: AbortSignal
) {
	return (await query(GuestsDocument, { propertyId, search, first }, signal)).guests;
}

/** Rooms of `roomTypeId` free for `[checkIn, checkOut)`: candidates for the assign picker. */
export async function fetchFreeRooms(
	propertyId: string,
	roomTypeId: string,
	checkIn: string,
	checkOut: string,
	signal?: AbortSignal
) {
	return (await query(FreeRoomsDocument, { propertyId, roomTypeId, checkIn, checkOut }, signal))
		.freeRooms;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** `YYYY-MM-DD` split into numbers, with no time zone involved. */
function splitDate(date: string): [number, number, number] {
	const [year, month, day] = date.split('-').map(Number);
	return [year, month, day];
}

/**
 * A stay's dates and length, e.g. `3 Oct – 5 Oct 2026 · 2 nights`. The year is shown once, at the end,
 * unless the stay crosses a new year, in which case both dates carry their own year.
 */
export function formatStay(arrival: string, departure: string): string {
	const [arrivalYear, arrivalMonth, arrivalDay] = splitDate(arrival);
	const [departureYear, departureMonth, departureDay] = splitDate(departure);
	const sameYear = arrivalYear === departureYear;
	const nights = Math.round(
		(Date.UTC(departureYear, departureMonth - 1, departureDay) -
			Date.UTC(arrivalYear, arrivalMonth - 1, arrivalDay)) /
			86_400_000
	);
	const from = `${arrivalDay} ${MONTHS[arrivalMonth - 1]}${sameYear ? '' : ` ${arrivalYear}`}`;
	const to = `${departureDay} ${MONTHS[departureMonth - 1]} ${departureYear}`;
	return `${from} – ${to} · ${nights} night${nights === 1 ? '' : 's'}`;
}

const STATUS_LABELS: Record<RoomStatus, string> = {
	TENTATIVE: 'Tentative',
	CONFIRMED: 'Confirmed',
	CHECKED_IN: 'Checked in',
	CHECKED_OUT: 'Checked out',
	CANCELLED: 'Cancelled',
	NO_SHOW: 'No-show'
};

/** How a room's (or a reservation's derived) status reads in the UI. */
export function statusLabel(status: RoomStatus): string {
	return STATUS_LABELS[status];
}

/** Every room status, in the order the table's filter lists them. */
export const STATUSES: readonly RoomStatus[] = [
	'TENTATIVE',
	'CONFIRMED',
	'CHECKED_IN',
	'CHECKED_OUT',
	'CANCELLED',
	'NO_SHOW'
];

const SOURCE_LABELS: Record<Source, string> = {
	FRONT_DESK: 'Front desk',
	PHONE: 'Phone',
	EMAIL: 'Email',
	IBE: 'Booking engine',
	CHANNEL: 'Channel'
};

/** Every source, in the order the table's filter lists them. */
export const SOURCES: readonly Source[] = ['FRONT_DESK', 'PHONE', 'EMAIL', 'IBE', 'CHANNEL'];

/** How a reservation's source reads in the UI. */
export function sourceLabel(source: Source): string {
	return SOURCE_LABELS[source];
}

/**
 * A checkbox group's filter after one box changes. `chosen` left out means every choice (no filter); the
 * result is `undefined` again once every choice is on, otherwise the chosen values in `all`'s order, and
 * `[]` (matches nothing) once every box is off.
 */
export function toggleChoice<T>(
	all: readonly T[],
	chosen: readonly T[] | null | undefined,
	value: T,
	on: boolean
): T[] | undefined {
	const set = new Set(chosen ?? all);
	if (on) set.add(value);
	else set.delete(value);
	const next = all.filter((choice) => set.has(choice));
	return next.length === all.length ? undefined : next;
}

/** Every violation's message, joined the way the server joins them in a 422 (`"; "`). */
export function violationsText(violations: readonly { message: string }[]): string {
	return violations.map((violation) => violation.message).join('; ');
}

const MEAL_PLAN_NAMES: Record<MealPlan, string> = {
	RO: 'Room only',
	BB: 'Bed & breakfast',
	HB: 'Half board',
	FB: 'Full board'
};

/** An offer's plan and meal plan, e.g. `BAR · Half board`. */
export function offerLabel(offer: Pick<Offer, 'ratePlanCode' | 'mealPlan'>): string {
	return `${offer.ratePlanCode} · ${MEAL_PLAN_NAMES[offer.mealPlan]}`;
}

/** One room type × rate plan × meal plan combination, flattened for the new-reservation offers list. */
export interface OfferRow {
	roomTypeId: string;
	roomTypeCode: string;
	roomTypeName: string;
	free: number;
	ratePlanId: string;
	ratePlanCode: string;
	mealPlan: MealPlan;
	label: string;
	total: number;
	currency: string;
	totalLabel: string;
	/** Whether this offer can be taken as quoted: the plan's restrictions allow it and a room is free. */
	sellable: boolean;
	/** Why it can't be sold, when `sellable` is false; empty otherwise. */
	violations: string;
}

/** Flattens `availability` into one row per offer, for the new-reservation screen's list. */
export function groupOffers(availability: readonly RoomTypeAvailability[]): OfferRow[] {
	return availability.flatMap((type) =>
		type.offers.map((offer) => ({
			roomTypeId: type.roomTypeId,
			roomTypeCode: type.code,
			roomTypeName: type.name,
			free: type.free,
			ratePlanId: offer.ratePlanId,
			ratePlanCode: offer.ratePlanCode,
			mealPlan: offer.mealPlan,
			label: offerLabel(offer),
			total: offer.total,
			currency: offer.currency,
			totalLabel: formatMoney(offer.total, offer.currency),
			sellable: offer.restrictionsOk && type.free > 0,
			violations: violationsText(offer.violations)
		}))
	);
}

/** Comma-separated list params, split on commas and stripped of empty entries; `undefined` when absent. */
function listParam(params: URLSearchParams, name: string): string[] | undefined {
	if (!params.has(name)) return undefined;
	return (params.get(name) ?? '').split(',').filter((value) => value !== '');
}

/** Type guard: `value` is one of the allowed values. */
const oneOf =
	<T extends string>(allowed: readonly T[]) =>
	(value: string): value is T =>
		(allowed as readonly string[]).includes(value);

const isRoomStatus = oneOf([
	'CANCELLED',
	'CHECKED_IN',
	'CHECKED_OUT',
	'CONFIRMED',
	'NO_SHOW',
	'TENTATIVE'
] as const);
const isSource = oneOf(['CHANNEL', 'EMAIL', 'FRONT_DESK', 'IBE', 'PHONE'] as const);
const isSortField = oneOf(['ARRIVAL', 'CONFIRMATION', 'CREATED', 'GUEST'] as const);
const isSortDirection = oneOf(['ASC', 'DESC'] as const);

/**
 * The list's filter and sort out of the URL's search params, so a link or a reload keeps them. Unknown
 * params are ignored; a param equal to the default is treated the same as it being absent. Unknown enum
 * values are dropped; the sort field and direction fall back to defaults when unknown.
 */
export function filterFromSearchParams(params: URLSearchParams): ReservationListParams {
	const arrivalFrom = params.get('arrivalFrom');
	const arrivalTo = params.get('arrivalTo');
	const statusesRaw = listParam(params, 'statuses');
	const sourcesRaw = listParam(params, 'sources');
	const text = params.get('text');
	const field = params.get('sort');
	const direction = params.get('dir');

	const statuses =
		statusesRaw !== undefined
			? (statusesRaw.filter((s) => isRoomStatus(s)) as RoomStatus[])
			: undefined;
	const sources =
		sourcesRaw !== undefined ? (sourcesRaw.filter((s) => isSource(s)) as Source[]) : undefined;

	const filter: ReservationFilter = {
		...(arrivalFrom ? { arrivalFrom } : {}),
		...(arrivalTo ? { arrivalTo } : {}),
		...(statuses !== undefined ? { statuses } : {}),
		...(sources !== undefined ? { sources } : {}),
		...(text ? { text } : {})
	};

	return {
		filter,
		sort: {
			field: field && isSortField(field) ? field : DEFAULT_SORT.field,
			direction: direction && isSortDirection(direction) ? direction : DEFAULT_SORT.direction
		}
	};
}

/**
 * The list's filter and sort as URL search params, in a stable order, with anything equal to the default
 * left out (so the default list has no search string at all).
 */
export function filterToSearchParams(params: ReservationListParams): URLSearchParams {
	const search = new URLSearchParams();
	const { filter, sort } = params;
	if (filter.arrivalFrom) search.set('arrivalFrom', filter.arrivalFrom);
	if (filter.arrivalTo) search.set('arrivalTo', filter.arrivalTo);
	if (Array.isArray(filter.statuses)) search.set('statuses', filter.statuses.join(','));
	if (Array.isArray(filter.sources)) search.set('sources', filter.sources.join(','));
	if (filter.text) search.set('text', filter.text);
	if (sort.field !== DEFAULT_SORT.field) search.set('sort', sort.field);
	if (sort.direction !== DEFAULT_SORT.direction) search.set('dir', sort.direction);
	return search;
}
