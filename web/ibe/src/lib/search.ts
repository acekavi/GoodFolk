import {
	publicApi,
	TOO_MANY_TRIES,
	type Residency,
	type SearchResult,
	type SiteRoomType
} from './api';
import { addDays, addMonths, nights } from './format';

/** The site's limits, as the server enforces them (`MAX_ROOMS`, `MAX_NIGHTS`, `MONTHS_AHEAD`). */
export const MAX_ROOMS = 4;
export const MAX_NIGHTS = 30;
export const MONTHS_AHEAD = 18;

/** The question every search starts with: no price is shown before it is answered. */
export const RESIDENCY_QUESTION = 'Are you a resident of Sri Lanka?';

export interface RoomGuests {
	adults: number;
	children: number;
}

/** A stay to search, as the results page's address carries it. */
export interface Stay {
	checkIn: string;
	checkOut: string;
	residency: Residency;
	rooms: RoomGuests[];
}

/** The search form's state: no residency until the guest answers. */
export interface SearchForm {
	checkIn: string;
	checkOut: string;
	residency: Residency | null;
	rooms: RoomGuests[];
}

/** Rooms as the API's `rooms` parameter: `2-0,2-1`. */
export function roomsParam(rooms: readonly RoomGuests[]): string {
	return rooms.map((room) => `${room.adults}-${room.children}`).join(',');
}

/** `2-0,2-1` as rooms, or `null` unless it is 1 to 4 rooms of at least one adult each. */
export function parseRooms(param: string | null): RoomGuests[] | null {
	if (!param) return null;
	const rooms: RoomGuests[] = [];
	for (const room of param.split(',')) {
		const match = /^(\d{1,2})-(\d{1,2})$/.exec(room);
		if (!match) return null;
		rooms.push({ adults: Number(match[1]), children: Number(match[2]) });
	}
	return rooms.length <= MAX_ROOMS && rooms.every((room) => room.adults >= 1) ? rooms : null;
}

const DATE = /^\d{4}-\d{2}-\d{2}$/;

/** The stay's query string, in the API's own parameter names. */
export function stayQuery(stay: Stay): URLSearchParams {
	return new URLSearchParams({
		check_in: stay.checkIn,
		check_out: stay.checkOut,
		residency: stay.residency,
		rooms: roomsParam(stay.rooms)
	});
}

/** What a page's address says of the stay; the residency is `null` until the guest has answered. */
export function formFromUrl(url: URL, businessDate: string): SearchForm {
	const checkIn = url.searchParams.get('check_in') ?? '';
	const checkOut = url.searchParams.get('check_out') ?? '';
	const residency = url.searchParams.get('residency');
	const validIn = DATE.test(checkIn) ? checkIn : businessDate;
	return {
		checkIn: validIn,
		checkOut: DATE.test(checkOut) && checkOut > validIn ? checkOut : addDays(validIn, 1),
		residency: residency === 'resident' || residency === 'non_resident' ? residency : null,
		rooms: parseRooms(url.searchParams.get('rooms')) ?? [{ adults: 2, children: 0 }]
	};
}

/** Why the form's stay cannot be searched, or `null` when it can. */
export function stayProblem(form: SearchForm, businessDate: string): string | null {
	if (form.residency === null) return `Answer “${RESIDENCY_QUESTION}” to see prices`;
	if (!DATE.test(form.checkIn) || !DATE.test(form.checkOut)) return 'Choose your dates';
	if (form.checkIn < businessDate) return 'Check-in cannot be in the past';
	if (form.checkIn > addMonths(businessDate, MONTHS_AHEAD))
		return `Check-in can be up to ${MONTHS_AHEAD} months ahead`;
	const stayNights = nights(form.checkIn, form.checkOut);
	if (stayNights < 1) return 'Check-out must be after check-in';
	if (stayNights > MAX_NIGHTS) return `A stay can be up to ${MAX_NIGHTS} nights`;
	if (form.rooms.length < 1 || form.rooms.length > MAX_ROOMS) return `Book 1 to ${MAX_ROOMS} rooms`;
	return null;
}

/** The form as a stay to search; `null` while it cannot be searched, above all before the residency is answered. */
export function stayOf(form: SearchForm, businessDate: string): Stay | null {
	if (stayProblem(form, businessDate) !== null || form.residency === null) return null;
	return { ...form, residency: form.residency, rooms: form.rooms.map((room) => ({ ...room })) };
}

/** How many guests the site's rooms can hold, for the occupancy picker. */
export interface OccupancyLimits {
	maxAdults: number;
	maxChildren: number;
}

/** The most adults and children any one room type holds. */
export function occupancyLimits(roomTypes: readonly SiteRoomType[]): OccupancyLimits {
	return {
		maxAdults: Math.max(1, ...roomTypes.map((type) => type.max_adults)),
		maxChildren: Math.max(0, ...roomTypes.map((type) => type.max_children))
	};
}

/** Whether some room type holds `guests`: within its adults, its children and its total occupancy. */
export function fitsSomeRoom(roomTypes: readonly SiteRoomType[], guests: RoomGuests): boolean {
	return roomTypes.some(
		(type) =>
			guests.adults <= type.max_adults &&
			guests.children <= type.max_children &&
			guests.adults + guests.children <= type.max_occupancy
	);
}

/** What the picker allows next for one room: a room has at least one adult and always fits some room type. */
export function pickerSteps(roomTypes: readonly SiteRoomType[], guests: RoomGuests) {
	return {
		addAdult: fitsSomeRoom(roomTypes, { ...guests, adults: guests.adults + 1 }),
		removeAdult: guests.adults > 1,
		addChild: fitsSomeRoom(roomTypes, { ...guests, children: guests.children + 1 }),
		removeChild: guests.children > 0
	};
}

/** Whether another room can be added to the search. */
export function canAddRoom(rooms: readonly RoomGuests[]): boolean {
	return rooms.length < MAX_ROOMS;
}

/** A search's outcome for the results page. */
export type Searched =
	| { kind: 'unanswered' }
	| { kind: 'found'; stay: Stay; result: SearchResult }
	| { kind: 'refused'; message: string };

/**
 * The results at `resultsHref` again after a room was just sold, marked `sold_out` with `nonce`, new for each time
 * (the booking attempt's key, say): see `searchFromUrl`.
 */
export function soldOutHref(resultsHref: string, nonce: string): string {
	return `${resultsHref}&sold_out=${encodeURIComponent(nonce)}`;
}

/**
 * Searches the stay a page's address carries. Without an answer to the residency question, or with a stay the site
 * cannot sell, nothing is sent: the guest is asked first. After a room was just sold, the address's `sold_out` nonce
 * goes to the API as `fresh`, so the query is new to the edge, whose cached answer of up to 30 s ago may still offer
 * the room that sold; every other search shares the edge's cache.
 */
export async function searchFromUrl(
	fetch: typeof globalThis.fetch,
	code: string,
	url: URL,
	businessDate: string
): Promise<Searched> {
	const form = formFromUrl(url, businessDate);
	const asked = ['check_in', 'check_out', 'rooms'].every((name) => url.searchParams.has(name));
	if (form.residency === null || !asked) return { kind: 'unanswered' };
	const stay = stayOf(form, businessDate);
	if (stay === null) return { kind: 'refused', message: stayProblem(form, businessDate) ?? '' };
	const fresh = url.searchParams.get('sold_out');
	const { data, error, response } = await publicApi(fetch).GET('/api/public/v1/ibe/{code}/search', {
		params: {
			path: { code },
			query: {
				check_in: stay.checkIn,
				check_out: stay.checkOut,
				residency: stay.residency,
				rooms: roomsParam(stay.rooms),
				...(fresh && { fresh })
			}
		}
	});
	if (data) return { kind: 'found', stay, result: data };
	if (response.status === 429) return { kind: 'refused', message: TOO_MANY_TRIES };
	const detail = (error as { detail?: string } | undefined)?.detail;
	if (response.status === 400 || response.status === 422)
		return { kind: 'refused', message: detail ?? 'These dates cannot be searched' };
	throw new Error(`the search answered ${response.status}`);
}

/** The same nights and rooms from another check-in: what "Try <date>" searches. */
export function movedTo(stay: Stay, checkIn: string): Stay {
	return { ...stay, checkIn, checkOut: addDays(checkIn, nights(stay.checkIn, stay.checkOut)) };
}
