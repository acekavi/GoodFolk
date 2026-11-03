import type { BookingBody, MealOffer, PlanOffer, RoomTypeResult, SearchResult } from './api';
import { isProblem } from './api';
import { dateText, money } from './format';
import { stayQuery, type Stay } from './search';

/** The guest's choice for one room asked: a room type, one of its plans and one of the plan's meal plans. */
export interface Pick {
	roomTypeId: string;
	ratePlanId: string;
	mealPlan: string;
}

/** A pick as the book page's address carries it, one `pick` parameter per room in order: `<type>~<plan>~<meal>`. */
export function pickParam(pick: Pick): string {
	return `${pick.roomTypeId}~${pick.ratePlanId}~${pick.mealPlan}`;
}

/** The book page for `stay` with one pick per room. */
export function bookHref(base: string, stay: Stay, picks: readonly Pick[]): string {
	const query = stayQuery(stay);
	for (const pick of picks) query.append('pick', pickParam(pick));
	return `${base}/book?${query}`;
}

/** The picks of a book page's address, or `null` unless there is one well-formed pick per room asked. */
export function picksFromUrl(url: URL, roomCount: number): Pick[] | null {
	const picks = url.searchParams.getAll('pick').map((param) => {
		const [roomTypeId, ratePlanId, mealPlan, ...rest] = param.split('~');
		return roomTypeId && ratePlanId && mealPlan && rest.length === 0
			? { roomTypeId, ratePlanId, mealPlan }
			: null;
	});
	if (picks.length !== roomCount || picks.some((pick) => pick === null)) return null;
	return picks as Pick[];
}

/** One room's priced choice. */
export interface PricedRoom {
	pick: Pick;
	roomType: RoomTypeResult;
	plan: PlanOffer;
	offer: MealOffer;
}

/**
 * The offer of the search that prices each pick for its room (the meal offer whose `rooms` holds the room's index),
 * or `null` when one is no longer offered, or a room type is picked for more rooms than it has free.
 */
export function pricePicks(result: SearchResult, picks: readonly Pick[]): PricedRoom[] | null {
	const priced: PricedRoom[] = [];
	for (const [index, pick] of picks.entries()) {
		const roomType = result.room_types.find((type) => type.room_type_id === pick.roomTypeId);
		const plan = roomType?.plans.find((offered) => offered.rate_plan_id === pick.ratePlanId);
		const offer = plan?.meal_plans.find(
			(meal) => meal.meal_plan === pick.mealPlan && meal.rooms.includes(index)
		);
		if (!roomType || !plan || !offer) return null;
		priced.push({ pick, roomType, plan, offer });
	}
	for (const roomType of new Set(priced.map((room) => room.roomType))) {
		if (priced.filter((room) => room.roomType === roomType).length > roomType.available)
			return null;
	}
	return priced;
}

/** The total per currency the guest is shown and agrees to, in minor units. */
export function totalsOf(rooms: readonly PricedRoom[]): Record<string, number> {
	const totals: Record<string, number> = {};
	for (const room of rooms) {
		totals[room.plan.currency] = (totals[room.plan.currency] ?? 0) + room.offer.total;
	}
	return totals;
}

/**
 * Whether `rooms`' prices add up to `expected`, the totals the guest is agreeing to. After a price change the page
 * searches again; until that search shows the new prices (or if it shows yet others), its per-room lines and deposit
 * are not the agreed ones, and only the totals are shown.
 */
export function pricesAgree(
	rooms: readonly PricedRoom[],
	expected: Record<string, number>
): boolean {
	const totals = totalsOf(rooms);
	const currencies = new Set([...Object.keys(totals), ...Object.keys(expected)]);
	return [...currencies].every((currency) => totals[currency] === expected[currency]);
}

/** An advisory deposit in one currency, in minor units. */
export interface DepositLine {
	currency: string;
	amount: number;
	dueDate: string;
}

/**
 * The booking's advisory deposit per currency, as the server adds up the booked rooms' (`deposit_lines`): each room's
 * deposit summed in its plan's currency, in the order the rooms first use them, due on the earliest of their dates.
 */
export function depositLines(rooms: readonly PricedRoom[]): DepositLine[] {
	const lines: DepositLine[] = [];
	for (const room of rooms) {
		const deposit = room.offer.deposit;
		if (!deposit) continue;
		const line = lines.find((each) => each.currency === room.plan.currency);
		if (line) {
			line.amount += deposit.amount;
			if (deposit.due_date < line.dueDate) line.dueDate = deposit.due_date;
		} else {
			lines.push({
				currency: room.plan.currency,
				amount: deposit.amount,
				dueDate: deposit.due_date
			});
		}
	}
	return lines;
}

/**
 * The deposit as the confirmation email words it: `Deposit (before taxes): US$ 120.00 by Mon, 2 Nov 2026. The hotel will contact you
 * about the deposit.`; `null` without one.
 */
export function depositText(lines: readonly DepositLine[]): string | null {
	if (!lines.length) return null;
	const owed = lines.map(
		(line) => `${money(line.amount, line.currency)} by ${dateText(line.dueDate)}`
	);
	return `Deposit (before taxes): ${owed.join(' and ')}. The hotel will contact you about the deposit.`;
}

/** What the guest fills in on the book page. */
export interface Details {
	firstName: string;
	lastName: string;
	email: string;
	phone: string;
	/** ISO 3166-1 alpha-2. */
	country: string;
	/** `HH:MM`, or empty. */
	arrivalTime: string;
	/** Per room, in order; empty for the booker. */
	occupants: string[];
	specialRequests: string;
	agreed: boolean;
}

/**
 * The booking to send for `stay` and its priced rooms. `expectedTotals` is what the guest saw and agreed to: the
 * search's totals, or the new ones after a price change. The honeypot `website` goes as it was found. An optional
 * field left empty is left out: the API refuses `null` in them.
 */
export function bookingPayload(
	stay: Stay,
	rooms: readonly PricedRoom[],
	details: Details,
	expectedTotals: Record<string, number>,
	turnstileToken: string,
	website: string
): BookingBody {
	return {
		residency: stay.residency,
		check_in: stay.checkIn,
		check_out: stay.checkOut,
		booker: {
			first_name: details.firstName.trim(),
			last_name: details.lastName.trim(),
			email: details.email.trim(),
			phone: details.phone.trim(),
			country: details.country,
			...(details.arrivalTime && { arrival_time: details.arrivalTime })
		},
		rooms: rooms.map((room, index) => {
			const occupant = details.occupants[index]?.trim();
			return {
				room_type_id: room.pick.roomTypeId,
				rate_plan_id: room.pick.ratePlanId,
				meal_plan: room.pick.mealPlan,
				adults: stay.rooms[index].adults,
				children: stay.rooms[index].children,
				...(occupant && { occupant_name: occupant })
			};
		}),
		special_requests: details.specialRequests.trim(),
		expected_totals: { ...expectedTotals },
		agreed: details.agreed,
		turnstile_token: turnstileToken,
		website
	};
}

/** What a booking attempt came to, from the API's answer. */
export type Outcome =
	| { kind: 'booked'; confirmationNo: string; manageUrl: string | null }
	| { kind: 'price_changed'; totals: Record<string, number> }
	| { kind: 'sold_out' }
	| { kind: 'in_progress' }
	| { kind: 'too_many'; retryAfter: number | null }
	| { kind: 'bot_check' }
	| { kind: 'invalid'; message: string }
	| { kind: 'failed' };

/**
 * Reads the API's answer to a booking. A `failed` attempt may be retried with the same key and body, and so may one
 * `in_progress`: an earlier attempt with the key (whose answer was lost) is still being worked on.
 */
export function bookingOutcome(status: number, body: unknown, retryAfter: string | null): Outcome {
	if (status === 201 && typeof body === 'object' && body !== null) {
		const confirmed = body as { confirmation_no: string; manage_url?: string | null };
		return {
			kind: 'booked',
			confirmationNo: confirmed.confirmation_no,
			manageUrl: confirmed.manage_url ?? null
		};
	}
	const problem = isProblem(body) ? body : null;
	if (status === 409 && problem?.reason === 'price_changed' && problem.totals)
		return { kind: 'price_changed', totals: problem.totals as Record<string, number> };
	if (status === 409 && problem?.reason === 'sold_out') return { kind: 'sold_out' };
	if (status === 409 && problem?.reason === 'in_progress') return { kind: 'in_progress' };
	if (status === 429) {
		const seconds = Number(retryAfter);
		return {
			kind: 'too_many',
			retryAfter: Number.isFinite(seconds) && seconds > 0 ? seconds : null
		};
	}
	if (status === 400 && /bot check/i.test(problem?.detail ?? '')) return { kind: 'bot_check' };
	if (status >= 400 && status < 500)
		return { kind: 'invalid', message: problem?.detail ?? 'The booking could not be made' };
	return { kind: 'failed' };
}

/**
 * The `Idempotency-Key` of booking attempts. One booking keeps its key until the API has answered it: a retry after a
 * network failure, a `5xx` or a `409` still in progress sends the same booking with the same key, even with a fresh
 * Turnstile token, which core-api leaves out of what it compares, so a first attempt that got through is replayed,
 * never booked twice. Any other answer (a changed price among them) ends the key, and a booking with other details
 * gets a new one.
 */
export function attemptKeys(newKey: () => string = () => crypto.randomUUID()) {
	let key: string | null = null;
	let booking = '';
	return {
		keyFor(payload: BookingBody): string {
			const serialized = JSON.stringify({ ...payload, turnstile_token: undefined });
			if (key === null || serialized !== booking) key = newKey();
			booking = serialized;
			return key;
		},
		answered(outcome: Outcome) {
			if (outcome.kind !== 'failed' && outcome.kind !== 'in_progress') key = null;
		}
	};
}

/** The API's answer to one booking request. */
export interface Answer {
	status: number;
	body: unknown;
	retryAfter: string | null;
}

/** The pauses before each retry of a booking still in progress, in milliseconds: five retries over 5 s. */
export const IN_PROGRESS_RETRIES = [500, 750, 1000, 1250, 1500];

/**
 * Sends a booking with `key` and reads the answer. While an earlier attempt with the same key is still in progress
 * (its answer was lost, and the guest tapped again), it is asked again with the same key after each of `pauses`, so
 * the guest gets that attempt's answer; after the last, the outcome is still `in_progress`, and the key is kept. A
 * failure to reach the API is thrown, as `send` throws it.
 */
export async function sendBooking(
	key: string,
	send: (key: string) => Promise<Answer>,
	pauses: readonly number[] = IN_PROGRESS_RETRIES,
	wait: (ms: number) => Promise<void> = (ms) => new Promise((done) => setTimeout(done, ms))
): Promise<Outcome> {
	for (let attempt = 0; ; attempt++) {
		const { status, body, retryAfter } = await send(key);
		const outcome = bookingOutcome(status, body, retryAfter);
		if (outcome.kind !== 'in_progress' || attempt === pauses.length) return outcome;
		await wait(pauses[attempt]);
	}
}

/** A booking just made, handed from the book page to its confirmation page in memory only, never stored. */
export interface Booked {
	confirmationNo: string;
	manageUrl: string | null;
	stay: Stay;
	rooms: { name: string; plan: string; mealPlan: string; currency: string; total: number }[];
	totals: Record<string, number>;
	email: string;
}

let lastBooked: Booked | null = null;

export function handOver(booked: Booked) {
	lastBooked = booked;
}

/** The booking just made with `confirmationNo`, once: a reload of the confirmation page shows the email's copy. */
export function takeBooked(confirmationNo: string): Booked | null {
	const booked = lastBooked?.confirmationNo === confirmationNo ? lastBooked : null;
	lastBooked = null;
	return booked;
}
