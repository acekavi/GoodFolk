import { describe, expect, it } from 'vitest';
import {
	availabilityKey,
	bookingStep,
	chooseGuest,
	createReservationBody,
	editStay,
	guestFromRest,
	guestsKey,
	NEW_BOOKING,
	nightsBetween,
	offerRefused,
	pickOffer,
	reservationListsKey,
	residencyLabel,
	roomsAllowed,
	searchAsGuest,
	searchStay,
	type Booking,
	type Guest,
	type OfferRow,
	type Stay,
	describePenalty,
	describeTerms,
	filterFromSearchParams,
	filterToSearchParams,
	formatStay,
	freeRoomsKey,
	groupOffers,
	historyLabel,
	idDocText,
	offerLabel,
	reservationKey,
	reservationsKey,
	SOURCES,
	sourceLabel,
	STATUSES,
	statusLabel,
	toggleChoice,
	violationsText,
	type CancellationTerms,
	type ReservationListParams,
	type RoomTypeAvailability
} from './reservations';

describe('keys', () => {
	it("the reservations list key starts with the server's reservations:<property> event key", () => {
		const key = reservationsKey('p1', {
			filter: { text: 'smith' },
			sort: { field: 'GUEST', direction: 'DESC' }
		});
		expect(key[0]).toBe('reservations:p1');
		expect(key).toEqual([
			'reservations:p1',
			{ filter: { text: 'smith' }, sort: { field: 'GUEST', direction: 'DESC' } }
		]);
	});

	it('the reservations key still starts with the same string with no params, so a bare event still matches', () => {
		expect(reservationsKey('p1')[0]).toBe('reservations:p1');
	});

	it('the list prefix key is the bare event key, so it matches every list of the property whatever its params', () => {
		expect(reservationListsKey('p1')).toEqual(['reservations:p1']);
		const list = reservationsKey('p1', {
			filter: { text: 'smith' },
			sort: { field: 'GUEST', direction: 'DESC' }
		});
		expect(list.slice(0, 1)).toEqual(reservationListsKey('p1'));
		expect(reservationsKey('p1').slice(0, 1)).toEqual(reservationListsKey('p1'));
	});

	it('the guests key carries the search under one prefix per property', () => {
		expect(guestsKey('p1', 'ada')).toEqual(['guests', 'p1', 'ada']);
		expect(guestsKey('p1')).toEqual(['guests', 'p1']);
	});

	it("the reservation detail key matches the server's reservation:<id> event key exactly", () => {
		expect(reservationKey('r1')).toEqual(['reservation:r1']);
	});

	it('the free rooms key carries the room type and the stay, under one prefix per property', () => {
		expect(freeRoomsKey('p1', 't1', '2026-10-03', '2026-10-05')).toEqual([
			'freeRooms',
			'p1',
			't1',
			'2026-10-03',
			'2026-10-05'
		]);
		expect(freeRoomsKey('p1')).toEqual(['freeRooms', 'p1']);
	});

	it('the availability key carries every argument that changes the answer', () => {
		expect(availabilityKey('p1', '2026-10-03', '2026-10-05', 2, 1, 'RESIDENT')).toEqual([
			'availability',
			'p1',
			'2026-10-03',
			'2026-10-05',
			2,
			1,
			'RESIDENT'
		]);
		expect(availabilityKey('p1')).toEqual(['availability', 'p1']);
	});
});

describe('search params round trip', () => {
	it('round-trips every filter and sort field', () => {
		const params: ReservationListParams = {
			filter: {
				arrivalFrom: '2026-10-01',
				arrivalTo: '2026-10-31',
				statuses: ['CONFIRMED', 'CHECKED_IN'],
				sources: ['FRONT_DESK'],
				text: 'GFK-0001'
			},
			sort: { field: 'GUEST', direction: 'DESC' }
		};

		const search = filterToSearchParams(params);

		expect(filterFromSearchParams(search)).toEqual(params);
	});

	it('omits fields at their default and produces no search string for the default list', () => {
		const search = filterToSearchParams({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});

		expect(search.toString()).toBe('');
		expect(filterFromSearchParams(new URLSearchParams())).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('keeps an explicitly empty selection distinct from no filter at all', () => {
		const params: ReservationListParams = {
			filter: { statuses: [], sources: [] },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		};

		const search = filterToSearchParams(params);

		expect(search.get('statuses')).toBe('');
		expect(filterFromSearchParams(search)).toEqual(params);
	});

	it('writes params in a stable order regardless of the object key order given to it', () => {
		const a = filterToSearchParams({
			filter: { text: 'x', arrivalFrom: '2026-01-01' },
			sort: { field: 'CREATED', direction: 'DESC' }
		});
		const b = filterToSearchParams({
			filter: { arrivalFrom: '2026-01-01', text: 'x' },
			sort: { direction: 'DESC', field: 'CREATED' }
		});

		expect(a.toString()).toBe(b.toString());
		expect([...a.keys()]).toEqual(['arrivalFrom', 'text', 'sort', 'dir']);
	});

	it('ignores params it does not recognise', () => {
		const search = new URLSearchParams('foo=bar&arrivalFrom=2026-10-01&utm_source=x');

		expect(filterFromSearchParams(search)).toEqual({
			filter: { arrivalFrom: '2026-10-01' },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('drops unknown enum values from statuses, keeping the known ones', () => {
		const search = new URLSearchParams('statuses=CONFIRMED,BOGUS,CHECKED_IN');

		expect(filterFromSearchParams(search)).toEqual({
			filter: { statuses: ['CONFIRMED', 'CHECKED_IN'] },
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('falls back to default sort field when the value is unknown', () => {
		const search = new URLSearchParams('sort=NOPE');

		expect(filterFromSearchParams(search)).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});

	it('falls back to default sort direction when the value is unknown', () => {
		const search = new URLSearchParams('dir=SIDEWAYS');

		expect(filterFromSearchParams(search)).toEqual({
			filter: {},
			sort: { field: 'ARRIVAL', direction: 'ASC' }
		});
	});
});

describe('formatStay', () => {
	it('shows the year once, at the end, within one calendar year', () => {
		expect(formatStay('2026-10-03', '2026-10-05')).toBe('3 Oct – 5 Oct 2026 · 2 nights');
	});

	it('crosses months within the same year', () => {
		expect(formatStay('2026-09-28', '2026-10-03')).toBe('28 Sep – 3 Oct 2026 · 5 nights');
	});

	it('carries the year on both ends when the stay crosses a new year', () => {
		expect(formatStay('2026-12-30', '2027-01-02')).toBe('30 Dec 2026 – 2 Jan 2027 · 3 nights');
	});

	it('says "1 night" in the singular', () => {
		expect(formatStay('2026-10-03', '2026-10-04')).toBe('3 Oct – 4 Oct 2026 · 1 night');
	});
});

describe('statusLabel', () => {
	it('reads every room status in plain words', () => {
		expect(statusLabel('TENTATIVE')).toBe('Tentative');
		expect(statusLabel('CHECKED_IN')).toBe('Checked in');
		expect(statusLabel('NO_SHOW')).toBe('No-show');
	});
});

describe('sourceLabel', () => {
	it('reads every source in plain words', () => {
		expect(SOURCES.map(sourceLabel)).toEqual([
			'Front desk',
			'Phone',
			'Email',
			'Booking engine',
			'Channel'
		]);
	});
});

describe('toggleChoice', () => {
	it('starts from every choice when nothing is chosen yet (no filter means any)', () => {
		expect(toggleChoice(STATUSES, undefined, 'CANCELLED', false)).toEqual([
			'TENTATIVE',
			'CONFIRMED',
			'CHECKED_IN',
			'CHECKED_OUT',
			'NO_SHOW'
		]);
	});

	it('goes back to no filter once every choice is on again', () => {
		expect(toggleChoice(SOURCES, ['PHONE', 'EMAIL', 'IBE', 'CHANNEL'], 'FRONT_DESK', true)).toBe(
			undefined
		);
	});

	it("keeps the choices in the list's order, and an empty choice matches nothing", () => {
		expect(toggleChoice(SOURCES, ['EMAIL'], 'PHONE', true)).toEqual(['PHONE', 'EMAIL']);
		expect(toggleChoice(SOURCES, ['EMAIL'], 'EMAIL', false)).toEqual([]);
	});
});

describe('violationsText', () => {
	it("joins every violation's message the way the server joins a 422", () => {
		expect(violationsText([{ message: 'closed' }, { message: 'below minimum stay' }])).toBe(
			'closed; below minimum stay'
		);
		expect(violationsText([])).toBe('');
	});
});

describe('offerLabel', () => {
	it('names the plan and the meal plan', () => {
		expect(offerLabel({ ratePlanCode: 'BAR', mealPlan: 'HB' })).toBe('BAR · Half board');
	});
});

describe('groupOffers', () => {
	it('flattens each room type into one row per offer, marking what cannot be sold', () => {
		const availability: RoomTypeAvailability[] = [
			{
				roomTypeId: 'dlx',
				code: 'DLX',
				name: 'Deluxe',
				free: 0,
				offers: [
					{
						ratePlanId: 'bar',
						ratePlanCode: 'BAR',
						mealPlan: 'RO',
						total: 10000,
						currency: 'USD',
						restrictionsOk: true,
						violations: [],
						nights: []
					}
				]
			},
			{
				roomTypeId: 'std',
				code: 'STD',
				name: 'Standard',
				free: 3,
				offers: [
					{
						ratePlanId: 'bar',
						ratePlanCode: 'BAR',
						mealPlan: 'BB',
						total: 12000,
						currency: 'USD',
						restrictionsOk: false,
						violations: [{ kind: 'CLOSED', message: 'closed' }],
						nights: []
					}
				]
			}
		];

		const rows = groupOffers(availability);

		expect(rows).toHaveLength(2);
		expect(rows[0]).toMatchObject({
			roomTypeId: 'dlx',
			label: 'BAR · Room only',
			totalLabel: '100.00',
			sellable: false // no rooms free, even though restrictionsOk
		});
		expect(rows[1]).toMatchObject({
			roomTypeId: 'std',
			sellable: false,
			violations: 'closed'
		});
	});
});

describe('describePenalty', () => {
	it('reads nights, percentages of the stay and amounts in the stay currency', () => {
		expect(describePenalty({ kind: 'NIGHTS', value: 1 }, 'USD')).toBe('1 night');
		expect(describePenalty({ kind: 'NIGHTS', value: 2 }, 'USD')).toBe('2 nights');
		expect(describePenalty({ kind: 'PERCENT', value: 10_000 }, 'USD')).toBe('100% of the stay');
		expect(describePenalty({ kind: 'PERCENT', value: 1_250 }, 'USD')).toBe('12.5% of the stay');
		expect(describePenalty({ kind: 'AMOUNT', value: 15_000 }, 'USD')).toBe('USD 150.00');
		expect(describePenalty({ kind: 'AMOUNT', value: 5_000 }, 'LKR')).toBe('LKR 50.00');
	});
});

describe('describeTerms', () => {
	const noShow = { kind: 'NIGHTS', value: 1 } as const;
	const terms = (rules: CancellationTerms['rules']): CancellationTerms => ({ rules, noShow });

	it('reads one rule as free until that many days before arrival, then its penalty', () => {
		expect(
			describeTerms(terms([{ daysBeforeArrival: 7, penalty: { kind: 'NIGHTS', value: 1 } }]), 'USD')
		).toBe('Free until 7 days before arrival, then 1 night');
	});

	it('reads later rules from the furthest from arrival to the nearest, whatever order they are stored in', () => {
		expect(
			describeTerms(
				terms([
					{ daysBeforeArrival: 0, penalty: { kind: 'PERCENT', value: 10_000 } },
					{ daysBeforeArrival: 30, penalty: { kind: 'AMOUNT', value: 5_000 } },
					{ daysBeforeArrival: 1, penalty: { kind: 'NIGHTS', value: 2 } }
				]),
				'USD'
			)
		).toBe(
			'Free until 30 days before arrival, then USD 50.00; from 1 day before arrival, 2 nights; from the day of arrival, 100% of the stay'
		);
	});

	it('says a stay is free to cancel when it has no terms or no rules', () => {
		expect(describeTerms(null, 'USD')).toBe('Free to cancel');
		expect(describeTerms(undefined, 'USD')).toBe('Free to cancel');
		expect(describeTerms(terms([]), 'USD')).toBe('Free to cancel');
	});
});

describe('historyLabel', () => {
	it('reads what was done, with the room number or the penalty the entry recorded', () => {
		expect(historyLabel({ action: 'reservation.created', data: {} })).toBe('Booked');
		expect(
			historyLabel({ action: 'reservation_room.assigned', data: { number: '102', previous: null } })
		).toBe('Room 102 assigned');
		expect(
			historyLabel({
				action: 'reservation_room.assigned',
				data: { number: '102', previous: '101' }
			})
		).toBe('Moved from room 101 to 102');
		expect(historyLabel({ action: 'reservation_room.unassigned', data: { number: '101' } })).toBe(
			'Room 101 unassigned'
		);
		expect(
			historyLabel({
				action: 'reservation_room.cancelled',
				data: { penalty: 10_000, currency: 'USD' }
			})
		).toBe('Room cancelled, costing USD 100.00');
		expect(
			historyLabel({ action: 'reservation_room.cancelled', data: { penalty: 0, currency: 'USD' } })
		).toBe('Room cancelled at no cost');
	});

	it('shows an action it does not know as it is', () => {
		expect(historyLabel({ action: 'reservation.noted', data: null })).toBe('reservation.noted');
	});
});

describe('idDocText', () => {
	it('names the document and shows only its masked number', () => {
		expect(idDocText({ idDocType: 'PASSPORT', idDocMasked: '•••• 5432' })).toBe(
			'Passport •••• 5432'
		);
		expect(idDocText({ idDocType: 'DRIVING_LICENCE', idDocMasked: '•••• 0001' })).toBe(
			'Driving licence •••• 0001'
		);
		expect(idDocText({ idDocType: null, idDocMasked: null })).toBe('None on file');
	});
});

describe('nightsBetween', () => {
	it('counts the nights of a stay, across a month and a year end', () => {
		expect(nightsBetween('2026-10-03', '2026-10-05')).toBe(2);
		expect(nightsBetween('2026-10-31', '2026-11-01')).toBe(1);
		expect(nightsBetween('2026-12-30', '2027-01-02')).toBe(3);
		expect(nightsBetween('2026-10-03', '2026-10-03')).toBe(0);
	});
});

describe('residencyLabel', () => {
	it('reads each residency', () => {
		expect(residencyLabel('RESIDENT')).toBe('Resident');
		expect(residencyLabel('NON_RESIDENT')).toBe('Non-resident');
	});
});

describe('guestFromRest', () => {
	it("reads a created guest as the search's guests read, masked ID included", () => {
		expect(
			guestFromRest({
				id: 'g1',
				first_name: 'Grace',
				last_name: 'Hopper',
				email: 'grace@example.com',
				phone: null,
				country: 'US',
				residency: 'non_resident',
				id_doc_type: 'driving_licence',
				id_doc_masked: '•••• 5432',
				notes: '',
				version: 1
			})
		).toEqual({
			id: 'g1',
			firstName: 'Grace',
			lastName: 'Hopper',
			email: 'grace@example.com',
			phone: null,
			country: 'US',
			residency: 'NON_RESIDENT',
			idDocType: 'DRIVING_LICENCE',
			idDocMasked: '•••• 5432',
			notes: '',
			version: 1
		});
	});

	it('leaves the ID out when the guest has none', () => {
		const guest = guestFromRest({
			id: 'g1',
			first_name: '',
			last_name: 'Madonna',
			residency: 'resident',
			notes: '',
			version: 1
		});
		expect(guest).toMatchObject({
			residency: 'RESIDENT',
			idDocType: null,
			idDocMasked: null,
			email: null
		});
	});
});

describe('the new-reservation flow', () => {
	const stay: Stay = {
		checkIn: '2026-10-03',
		checkOut: '2026-10-05',
		adults: 2,
		children: 0,
		residency: 'NON_RESIDENT'
	};
	const offer = (overrides: Partial<OfferRow> = {}): OfferRow => ({
		roomTypeId: 'dlx',
		roomTypeCode: 'DLX',
		roomTypeName: 'Deluxe',
		free: 3,
		ratePlanId: 'bar',
		ratePlanCode: 'BAR',
		mealPlan: 'RO',
		label: 'BAR · Room only',
		total: 20000,
		currency: 'USD',
		totalLabel: '200.00',
		sellable: true,
		violations: '',
		nights: [],
		...overrides
	});
	const guest = (residency: Guest['residency'], id = 'g1'): Guest => ({
		id,
		firstName: 'Ada',
		lastName: 'Silva',
		email: null,
		phone: null,
		country: null,
		residency,
		idDocType: null,
		idDocMasked: null,
		notes: '',
		version: 1
	});
	const booked = (): Booking =>
		chooseGuest(pickOffer(searchStay(stay), offer()), guest('NON_RESIDENT'));

	it('goes stay, offers, guest, review as each step is done', () => {
		expect(bookingStep(NEW_BOOKING)).toBe('stay');
		const searched = searchStay(stay);
		expect(bookingStep(searched)).toBe('offers');
		const picked = pickOffer(searched, offer());
		expect(bookingStep(picked)).toBe('guest');
		expect(bookingStep(chooseGuest(picked, guest('NON_RESIDENT')))).toBe('review');
	});

	it('editing the stay clears every later step', () => {
		expect(editStay(booked())).toEqual(NEW_BOOKING);
	});

	it('searching again clears the offer and the guest', () => {
		expect(searchStay({ ...stay, adults: 1 })).toEqual({
			...NEW_BOOKING,
			stay: { ...stay, adults: 1 }
		});
	});

	it('changing the chosen offer clears the guest', () => {
		const changed = pickOffer(booked(), offer({ mealPlan: 'BB' }));
		expect(changed.guest).toBeNull();
		expect(bookingStep(changed)).toBe('guest');
	});

	it('a guest of another residency than the stay is held back, not chosen', () => {
		const picked = pickOffer(searchStay(stay), offer());
		const mismatched = chooseGuest(picked, guest('RESIDENT'));
		expect(mismatched.guest).toBeNull();
		expect(mismatched.mismatch).toEqual(guest('RESIDENT'));
		expect(bookingStep(mismatched)).toBe('guest');
		// Choosing a matching guest instead drops the warning.
		expect(chooseGuest(mismatched, guest('NON_RESIDENT', 'g2'))).toMatchObject({
			guest: { id: 'g2' },
			mismatch: null
		});
	});

	it('searching again as the held-back guest prices the stay for their residency and keeps them for the review', () => {
		const picked = pickOffer(searchStay(stay), offer());
		const again = searchAsGuest(chooseGuest(picked, guest('RESIDENT')));
		expect(again).toEqual({
			stay: { ...stay, residency: 'RESIDENT' },
			offer: null,
			guest: guest('RESIDENT'),
			mismatch: null
		});
		expect(bookingStep(again)).toBe('offers');
		// Picking an offer for that stay goes straight to the review with the guest.
		expect(bookingStep(pickOffer(again, offer()))).toBe('review');
	});

	it('a refused booking goes back to the offers, keeping the stay and the guest', () => {
		const refused = offerRefused(booked());
		expect(refused).toMatchObject({ stay, offer: null, guest: guest('NON_RESIDENT') });
		expect(bookingStep(refused)).toBe('offers');
	});

	it('allows as many rooms as are free, up to the most one reservation takes', () => {
		expect(roomsAllowed(offer({ free: 3 }))).toBe(3);
		expect(roomsAllowed(offer({ free: 40 }))).toBe(10);
		expect(roomsAllowed(offer({ free: 0 }))).toBe(0);
		expect(roomsAllowed(offer({ free: -2 }))).toBe(0);
	});

	it('books one room line per room, each for the stay on the chosen offer, for the chosen guest', () => {
		expect(createReservationBody(booked(), 2, 'PHONE', '  late arrival  ')).toEqual({
			booker_guest_id: 'g1',
			source: 'phone',
			notes: 'late arrival',
			rooms: [1, 2].map(() => ({
				room_type_id: 'dlx',
				rate_plan_id: 'bar',
				meal_plan: 'RO',
				check_in: '2026-10-03',
				check_out: '2026-10-05',
				adults: 2,
				children: 0
			}))
		});
		expect(createReservationBody(booked(), 1, 'FRONT_DESK', '')).not.toHaveProperty('notes');
	});
});
