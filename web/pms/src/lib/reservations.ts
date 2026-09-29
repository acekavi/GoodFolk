import { graphql } from './api/gql';
import type {
	AvailabilityQuery,
	FreeRoomsQuery,
	GuestsQuery,
	IdDocType,
	MealPlan,
	PenaltyKind,
	Residency,
	ReservationListQuery,
	ReservationQuery,
	ReservationSortField,
	RoomStatus,
	SortDirection,
	Source
} from './api/gql/graphql';
import { query } from './api/graphql';
import type { components } from './api/openapi';
import { addDays } from './inventory';
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
				accountName
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
			account {
				id
				name
				kind
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
				occupants {
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
				checkedInAt
				checkedInBusinessDate
				checkedOutAt
				canCheckIn
				canUndoCheckIn
				canCheckOut
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
export type CancellationTerms = NonNullable<ReservationRoom['cancellationTerms']>;
export type HistoryEntry = ReservationDetail['history'][number];

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

/**
 * The prefix of every reservations list key of the property, whatever its filter and sort: the bare
 * `reservations:<property>` event key, for invalidating every list after a command changes reservations.
 */
export function reservationListsKey(propertyId: string) {
	return [`reservations:${propertyId}`] as const;
}

/** Query key shared with the server's `reservation:<id>` event. */
export function reservationKey(id: string) {
	return [`reservation:${id}`] as const;
}

/**
 * Query key for the assign picker's free rooms of one type for one stay. Not named by any server event;
 * `freeRoomsKey(propertyId)` alone is the prefix of every such key, for invalidating them all after a room
 * is assigned, unassigned or cancelled.
 */
export function freeRoomsKey(
	propertyId: string,
	...stay: [roomTypeId: string, checkIn: string, checkOut: string] | []
) {
	return ['freeRooms', propertyId, ...stay] as const;
}

/** Query key for one guest search. Not named by any server event; `guestsKey(propertyId)` alone is the
 * prefix of every search, for refetching them after a guest is added. */
export function guestsKey(propertyId: string, ...search: [search: string] | []) {
	return ['guests', propertyId, ...search] as const;
}

/** Query key for one availability lookup. Not named by any server event: a stay's offers are asked for
 * again (a new search, or after a booking), never kept, so every argument that changes the answer is part
 * of the key. `availabilityKey(propertyId)` alone is the prefix of every lookup of the property. */
export function availabilityKey(
	propertyId: string,
	...stay:
		[checkIn: string, checkOut: string, adults: number, children: number, residency: Residency] | []
) {
	return ['availability', propertyId, ...stay] as const;
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

/** The nights of a stay of `[checkIn, checkOut)`. */
export function nightsBetween(checkIn: string, checkOut: string): number {
	const [inYear, inMonth, inDay] = splitDate(checkIn);
	const [outYear, outMonth, outDay] = splitDate(checkOut);
	return Math.round(
		(Date.UTC(outYear, outMonth - 1, outDay) - Date.UTC(inYear, inMonth - 1, inDay)) / 86_400_000
	);
}

/**
 * The nights an early check-out on `businessDate` would release, oldest first: empty for a late
 * check-out (`businessDate >= stay.checkOut`). Mirrors the server's own rule (`check_out`): the stay
 * shortens to `[checkIn, max(businessDate, checkIn + 1))`, so every night from there up to the old
 * `checkOut` (exclusive) is released.
 */
export function nightsReleasedOnCheckout(
	stay: { checkIn: string; checkOut: string },
	businessDate: string
): string[] {
	if (businessDate >= stay.checkOut) return [];
	const earliestCheckOut = addDays(stay.checkIn, 1);
	const newCheckOut = businessDate > earliestCheckOut ? businessDate : earliestCheckOut;
	const released: string[] = [];
	for (let date = newCheckOut; date < stay.checkOut; date = addDays(date, 1)) {
		released.push(date);
	}
	return released;
}

/**
 * A stay's dates and length, e.g. `3 Oct – 5 Oct 2026 · 2 nights`. The year is shown once, at the end,
 * unless the stay crosses a new year, in which case both dates carry their own year.
 */
export function formatStay(arrival: string, departure: string): string {
	const [arrivalYear, arrivalMonth, arrivalDay] = splitDate(arrival);
	const [departureYear, departureMonth, departureDay] = splitDate(departure);
	const sameYear = arrivalYear === departureYear;
	const nights = nightsBetween(arrival, departure);
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

/** A penalty in words: `1 night`, `12.5% of the stay` (the value is basis points) or `USD 150.00`. */
export function describePenalty(
	penalty: { kind: PenaltyKind; value: number },
	currency: string
): string {
	switch (penalty.kind) {
		case 'NIGHTS':
			return `${penalty.value} night${penalty.value === 1 ? '' : 's'}`;
		case 'PERCENT':
			return `${penalty.value / 100}% of the stay`;
		case 'AMOUNT':
			return `${currency} ${formatMoney(penalty.value, currency)}`;
	}
}

function daysBefore(days: number): string {
	if (days === 0) return 'the day of arrival';
	return `${days} day${days === 1 ? '' : 's'} before arrival`;
}

/**
 * A room's cancellation terms in words, e.g. `Free until 7 days before arrival, then 1 night`. A rule costs
 * its penalty from its number of days before arrival on, until a rule nearer arrival takes over (the server's
 * `cancellation_penalty` applies the rule with the fewest days still at or above the days left), so the rules
 * read from the furthest from arrival to the nearest. No terms, or no rules, means cancelling is free.
 */
export function describeTerms(
	terms: CancellationTerms | null | undefined,
	currency: string
): string {
	const rules = (terms?.rules ?? []).toSorted((a, b) => b.daysBeforeArrival - a.daysBeforeArrival);
	if (rules.length === 0) return 'Free to cancel';
	const [first, ...later] = rules;
	return [
		`Free until ${daysBefore(first.daysBeforeArrival)}, then ${describePenalty(first.penalty, currency)}`,
		...later.map(
			(rule) =>
				`from ${daysBefore(rule.daysBeforeArrival)}, ${describePenalty(rule.penalty, currency)}`
		)
	].join('; ');
}

/** One history entry's action in words, with the room or the penalty its audit data recorded. */
export function historyLabel(entry: Pick<HistoryEntry, 'action' | 'data'>): string {
	const data = (entry.data ?? {}) as Record<string, unknown>;
	switch (entry.action) {
		case 'reservation.created':
			return 'Booked';
		case 'reservation_room.assigned':
			return data.previous
				? `Moved from room ${data.previous} to ${data.number}`
				: `Room ${data.number} assigned`;
		case 'reservation_room.unassigned':
			return `Room ${data.number} unassigned`;
		case 'reservation_room.cancelled': {
			const penalty = Number(data.penalty ?? 0);
			const currency = String(data.currency ?? '');
			return penalty > 0
				? `Room cancelled, costing ${currency} ${formatMoney(penalty, currency)}`
				: 'Room cancelled at no cost';
		}
		default:
			return entry.action;
	}
}

const ID_DOC_LABELS: Record<IdDocType, string> = {
	PASSPORT: 'Passport',
	NIC: 'NIC',
	DRIVING_LICENCE: 'Driving licence',
	OTHER: 'ID'
};

/** A guest's ID document as the UI shows it: its kind and the masked number, never the number itself. */
export function idDocText(guest: {
	idDocType?: IdDocType | null;
	idDocMasked?: string | null;
}): string {
	if (!guest.idDocType || !guest.idDocMasked) return 'None on file';
	return `${ID_DOC_LABELS[guest.idDocType]} ${guest.idDocMasked}`;
}

const RESIDENCY_LABELS: Record<Residency, string> = {
	RESIDENT: 'Resident',
	NON_RESIDENT: 'Non-resident'
};

/** How a guest's (or a stay's) residency reads in the UI. */
export function residencyLabel(residency: Residency): string {
	return RESIDENCY_LABELS[residency];
}

/** A guest as the REST API returns it (e.g. just created), read as the GraphQL guest search reads guests. */
export function guestFromRest(guest: components['schemas']['Guest']): Guest {
	return {
		id: guest.id,
		firstName: guest.first_name,
		lastName: guest.last_name,
		email: guest.email ?? null,
		phone: guest.phone ?? null,
		country: guest.country ?? null,
		residency: guest.residency.toUpperCase() as Residency,
		idDocType: (guest.id_doc_type?.toUpperCase() ?? null) as IdDocType | null,
		idDocMasked: guest.id_doc_masked ?? null,
		notes: guest.notes,
		version: guest.version
	};
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
	/** The quote's prices night by night. */
	nights: Offer['nights'];
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
			violations: violationsText(offer.violations),
			nights: offer.nights
		}))
	);
}

/**
 * The offer for `roomTypeId`'s `ratePlanId` and `mealPlan` in an `availability` result, if the stay
 * sells it. Used by the detail modal's Modify preview, which reprices for a room's own (unchangeable)
 * plan and meal plan on the new stay and possibly new type.
 */
export function findOffer(
	availability: readonly RoomTypeAvailability[],
	roomTypeId: string,
	ratePlanId: string,
	mealPlan: MealPlan
): Offer | undefined {
	return availability
		.find((type) => type.roomTypeId === roomTypeId)
		?.offers.find((offer) => offer.ratePlanId === ratePlanId && offer.mealPlan === mealPlan);
}

/** What a booked room's modify form changes it from. */
export interface ModifyRoomCurrent {
	checkIn: string;
	checkOut: string;
	roomTypeId: string;
	adults: number;
	children: number;
}

/** The modify form's draft: `ModifyRoomCurrent`'s fields as edited, plus the two pricing flags. */
export interface ModifyRoomDraft extends ModifyRoomCurrent {
	keepPrice: boolean;
	reprice: boolean;
}

/**
 * The `modify_reservation_room` request for `draft` against `current`: only the fields that actually
 * changed are sent (the server refuses an empty change unless `reprice` is set), `keep_price` and
 * `reprice` are always sent as the form's explicit choice.
 */
export function modifyRoomBody(
	current: ModifyRoomCurrent,
	draft: ModifyRoomDraft
): components['schemas']['ModifyRoomRequest'] {
	const body: components['schemas']['ModifyRoomRequest'] = {
		keep_price: draft.keepPrice,
		reprice: draft.reprice
	};
	if (draft.checkIn !== current.checkIn) body.check_in = draft.checkIn;
	if (draft.checkOut !== current.checkOut) body.check_out = draft.checkOut;
	if (draft.roomTypeId !== current.roomTypeId) body.room_type_id = draft.roomTypeId;
	if (draft.adults !== current.adults) body.adults = draft.adults;
	if (draft.children !== current.children) body.children = draft.children;
	return body;
}

/**
 * Whether `modifyRoomBody(current, draft)` would actually change something, the same way the server decides
 * (`modify_reservation_room`'s "nothing to change"): a field differs from `current`, or `reprice` itself is
 * set. `keepPrice` alone changes nothing by itself. The modify form's Save button stays disabled while this
 * is `false`.
 */
export function modifyRoomHasChanges(current: ModifyRoomCurrent, draft: ModifyRoomDraft): boolean {
	return draft.reprice || Object.keys(modifyRoomBody(current, draft)).length > 2;
}

/** The most rooms one reservation takes (the server's `MAX_ROOMS_PER_RESERVATION`). */
export const MAX_ROOMS_PER_RESERVATION = 10;

/** What the new-reservation screen searches offers for: one room's stay and occupancy. */
export interface Stay {
	checkIn: string;
	/** The morning the guest leaves. */
	checkOut: string;
	adults: number;
	children: number;
	/** Prices the offers; the guest booked must have the same residency. */
	residency: Residency;
}

/**
 * The new-reservation screen's progress. Each step is done once its field is set, in order: the stay
 * searched, the offer picked, the guest chosen. Changing a step clears the steps after it, so what is
 * booked is always what was shown.
 */
export interface Booking {
	stay: Stay | null;
	offer: OfferRow | null;
	guest: Guest | null;
	/** A guest picked whose residency differs from the stay's, held back until the offers are searched
	 * again for their residency (the quote prices the stay for the guest's residency). */
	mismatch: Guest | null;
}

export type BookingStep = 'stay' | 'offers' | 'guest' | 'review';

export const NEW_BOOKING: Booking = { stay: null, offer: null, guest: null, mismatch: null };

/** The step waiting to be done. */
export function bookingStep(booking: Booking): BookingStep {
	if (!booking.stay) return 'stay';
	if (!booking.offer) return 'offers';
	if (!booking.guest) return 'guest';
	return 'review';
}

/** The stay searched: its offers come next, and nothing chosen for an earlier stay is kept. */
export function searchStay(stay: Stay): Booking {
	return { ...NEW_BOOKING, stay };
}

/** The stay being edited after its search: every later step is cleared until it is searched again. */
export function editStay(booking: Booking): Booking {
	return booking.stay ? NEW_BOOKING : booking;
}

/**
 * An offer picked. Changing an offer already picked clears the guest chosen after it; a guest kept by
 * `searchAsGuest` (chosen before this offer was) stays.
 */
export function pickOffer(booking: Booking, offer: OfferRow): Booking {
	return { ...booking, offer, guest: booking.offer ? null : booking.guest, mismatch: null };
}

/** A guest picked: chosen when their residency is the stay's, otherwise held back as a mismatch. */
export function chooseGuest(booking: Booking, guest: Guest): Booking {
	if (booking.stay && booking.stay.residency !== guest.residency) {
		return { ...booking, guest: null, mismatch: guest };
	}
	return { ...booking, guest, mismatch: null };
}

/** The offers searched again for the held-back guest's residency, keeping that guest for the review. */
export function searchAsGuest(booking: Booking): Booking {
	if (!booking.stay || !booking.mismatch) return booking;
	return {
		stay: { ...booking.stay, residency: booking.mismatch.residency },
		offer: null,
		guest: booking.mismatch,
		mismatch: null
	};
}

/** The booking refused (sold out meanwhile, or no longer sellable): back to the offers, keeping the
 * stay and the guest. */
export function offerRefused(booking: Booking): Booking {
	return { ...booking, offer: null };
}

/** How many rooms of an offer one reservation can take: those free, up to `MAX_ROOMS_PER_RESERVATION`. */
export function roomsAllowed(offer: Pick<OfferRow, 'free'>): number {
	return Math.max(0, Math.min(offer.free, MAX_ROOMS_PER_RESERVATION));
}

/**
 * The create request for a finished booking: `rooms` identical room lines on the chosen offer, each for
 * the stay, with the chosen guest as the booker (and so every room's guest). Empty notes are left out;
 * `accountId` is included as `account_id` only when a billing account was chosen.
 */
export function createReservationBody(
	booking: Booking,
	rooms: number,
	source: Source,
	notes: string,
	accountId?: string | null
): components['schemas']['CreateReservationRequest'] {
	const { stay, offer, guest } = booking;
	if (!stay || !offer || !guest) throw new Error('The booking is not finished.');
	const trimmed = notes.trim();
	return {
		booker_guest_id: guest.id,
		source: source.toLowerCase() as components['schemas']['Source'],
		...(trimmed ? { notes: trimmed } : {}),
		...(accountId ? { account_id: accountId } : {}),
		rooms: Array.from({ length: rooms }, () => ({
			room_type_id: offer.roomTypeId,
			rate_plan_id: offer.ratePlanId,
			meal_plan: offer.mealPlan,
			check_in: stay.checkIn,
			check_out: stay.checkOut,
			adults: stay.adults,
			children: stay.children
		}))
	};
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
