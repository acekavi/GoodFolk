<!--
	A new reservation, keyboard first, in four steps on one page: the stay (dates, occupancy, residency),
	an offer (room type × plan × meal plan, priced for the stay), the guest (found or added), and a review
	that creates it. Every step stays on screen and can be changed; changing one clears the steps after it
	(`Booking` in `$lib/reservations`). A booking takes one or more rooms of the offer taken, each for the
	same stay and guest. Once created, the new reservation opens in its modal over the table.
-->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { tick } from 'svelte';
	import type { Residency, Source } from '$lib/api/gql/graphql';
	import type { components } from '$lib/api/openapi';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import { addDays } from '$lib/inventory';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { formatMoney } from '$lib/rates';
	import {
		availabilityKey,
		bookingStep,
		chooseGuest,
		createReservationBody,
		editStay,
		fetchAvailability,
		fetchGuests,
		formatStay,
		groupOffers,
		guestFromRest,
		guestsKey,
		idDocText,
		NEW_BOOKING,
		nightsBetween,
		offerRefused,
		pickOffer,
		reservationListsKey,
		residencyLabel,
		roomsAllowed,
		searchAsGuest,
		searchStay,
		sourceLabel,
		type Booking,
		type Guest,
		type OfferRow
	} from '$lib/reservations';
	import { can, fetchMe } from '$lib/session';

	type IdDocType = components['schemas']['IdDocType'];

	/** The longest stay the offers are searched for (the server's availability limit). */
	const MAX_NIGHTS = 30;
	const SEARCH_DELAY_MS = 300;
	const RESIDENCIES: Residency[] = ['RESIDENT', 'NON_RESIDENT'];
	/** The sources a reservation is booked from here; the booking engine and channels book their own. */
	const SOURCES: Source[] = ['FRONT_DESK', 'PHONE', 'EMAIL'];
	const ID_DOC_TYPES: { value: IdDocType; label: string }[] = [
		{ value: 'passport', label: 'Passport' },
		{ value: 'nic', label: 'NIC' },
		{ value: 'driving_licence', label: 'Driving licence' },
		{ value: 'other', label: 'Other' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);

	let booking = $state.raw<Booking>(NEW_BOOKING);
	const pending = new Pending();

	let offersHeading = $state<HTMLElement>();
	let guestSearch = $state<HTMLInputElement>();
	let newGuestFirst = $state<HTMLInputElement>();
	let reviewHeading = $state<HTMLElement>();

	/** Moves the focus to a step's element once the step is on screen. */
	async function focus(target: () => HTMLElement | undefined) {
		await tick();
		target()?.focus();
	}

	// 1. The stay, as typed. Check-out and nights move together; a new check-in keeps the nights.
	let draft = $state({
		checkIn: '',
		checkOut: '',
		nights: 1,
		adults: 2,
		children: 0,
		residency: '' as Residency | ''
	});
	$effect(() => {
		if (businessDate && !draft.checkIn) {
			draft.checkIn = businessDate;
			draft.checkOut = addDays(businessDate, draft.nights);
		}
	});

	/** A change to the stay: the offers, guest and review found for the stay searched are cleared. */
	function changeStay(change: () => void) {
		change();
		booking = editStay(booking);
		refused = '';
	}

	function search(event: SubmitEvent) {
		event.preventDefault();
		if (!draft.residency) return;
		selected = '';
		refused = '';
		booking = searchStay({
			checkIn: draft.checkIn,
			checkOut: draft.checkOut,
			adults: draft.adults,
			children: draft.children,
			residency: draft.residency
		});
		void focus(() => offersHeading);
	}

	// 2. The offers for the stay searched, asked for afresh on every search: a room may have gone since.
	const availability = createQuery(() => {
		const stay = booking.stay;
		return {
			queryKey: stay
				? availabilityKey(
						propertyId,
						stay.checkIn,
						stay.checkOut,
						stay.adults,
						stay.children,
						stay.residency
					)
				: availabilityKey(propertyId),
			queryFn: ({ signal }: { signal: AbortSignal }) =>
				fetchAvailability(
					propertyId,
					stay!.checkIn,
					stay!.checkOut,
					stay!.adults,
					stay!.children,
					stay!.residency,
					signal
				),
			enabled: !!stay,
			staleTime: 0
		};
	});
	const offerGroups = $derived(
		(availability.data ?? []).map((type) => ({ type, rows: groupOffers([type]) }))
	);
	/** The offer radio chosen, as `offerKey`: arrow keys move it before an offer is taken. */
	let selected = $state('');
	/** Why the last create was refused (sold out meanwhile, or no longer sellable). */
	let refused = $state('');

	function offerKey(row: Pick<OfferRow, 'roomTypeId' | 'ratePlanId' | 'mealPlan'>): string {
		return `${row.roomTypeId}:${row.ratePlanId}:${row.mealPlan}`;
	}

	function take(row: OfferRow | undefined) {
		if (!row?.sellable) return;
		selected = offerKey(row);
		refused = '';
		if (booking.offer && offerKey(booking.offer) === selected) return;
		booking = pickOffer(booking, row);
		rooms = 1;
		void focus(() => (bookingStep(booking) === 'review' ? reviewHeading : guestSearch));
	}

	function takeSelected(event: SubmitEvent) {
		event.preventDefault();
		take(offerGroups.flatMap((group) => group.rows).find((row) => offerKey(row) === selected));
	}

	function selectOffer(row: OfferRow) {
		selected = offerKey(row);
		// Once an offer is taken, choosing another takes it (and clears the guest chosen after it).
		if (booking.offer) take(row);
	}

	// 3. The guest: found by name, email or phone (searched a moment after typing stops), or added.
	let guestText = $state('');
	let guestSearched = $state('');
	let guestTimer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => () => clearTimeout(guestTimer));
	const guests = createQuery(() => ({
		queryKey: guestsKey(propertyId, guestSearched),
		queryFn: ({ signal }) => fetchGuests(propertyId, guestSearched, 10, signal),
		enabled: !!booking.offer && guestSearched !== ''
	}));

	function findGuests() {
		clearTimeout(guestTimer);
		guestTimer = setTimeout(() => (guestSearched = guestText.trim()), SEARCH_DELAY_MS);
	}

	function findGuestsNow(event: SubmitEvent) {
		event.preventDefault();
		clearTimeout(guestTimer);
		guestSearched = guestText.trim();
	}

	function choose(guest: Guest) {
		booking = chooseGuest(booking, guest);
		if (booking.guest) void focus(() => reviewHeading);
	}

	function searchAgainAsGuest() {
		booking = searchAsGuest(booking);
		if (booking.stay) draft.residency = booking.stay.residency;
		selected = '';
		void focus(() => offersHeading);
	}

	interface GuestDraft {
		firstName: string;
		lastName: string;
		email: string;
		phone: string;
		country: string;
		residency: components['schemas']['Residency'];
		idType: IdDocType | '';
		idNumber: string;
	}

	let newGuest = $state<GuestDraft | null>(null);
	let guestError = $state('');
	const guestForm = formKeys();

	function openNewGuest() {
		guestError = '';
		newGuest = {
			firstName: '',
			lastName: '',
			email: '',
			phone: '',
			country: '',
			residency: booking.stay?.residency === 'RESIDENT' ? 'resident' : 'non_resident',
			idType: '',
			idNumber: ''
		};
		void focus(() => newGuestFirst);
	}

	async function addGuest(event: SubmitEvent) {
		event.preventDefault();
		if (!newGuest) return;
		const g = newGuest;
		guestError = '';
		const body: components['schemas']['CreateGuestRequest'] = {
			first_name: g.firstName.trim(),
			last_name: g.lastName.trim(),
			email: g.email.trim() || undefined,
			phone: g.phone.trim() || undefined,
			country: g.country.trim().toUpperCase() || undefined,
			residency: g.residency,
			id_doc: g.idType ? { type: g.idType, number: g.idNumber.trim() } : undefined
		};
		try {
			const created = await pending.run('guest', async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/guests', {
						params: {
							path: { property: propertyId },
							header: { 'Idempotency-Key': guestForm.keyFor(body) }
						},
						body
					})
				)
			);
			guestForm.reset();
			newGuest = null;
			void client.invalidateQueries({ queryKey: guestsKey(propertyId) });
			choose(guestFromRest(created));
		} catch (err) {
			guestForm.failed(err);
			guestError = errorMessage(err);
		}
	}

	// 4. The review, and the booking.
	let rooms = $state(1);
	let source = $state<Source>('FRONT_DESK');
	let notes = $state('');
	let createError = $state('');
	const createForm = formKeys();

	async function create(event: SubmitEvent) {
		event.preventDefault();
		createError = '';
		const body = createReservationBody(booking, rooms, source, notes);
		try {
			const created = await pending.run('create', async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/reservations', {
						params: {
							path: { property: propertyId },
							header: { 'Idempotency-Key': createForm.keyFor(body) }
						},
						body
					})
				)
			);
			createForm.reset();
			void client.invalidateQueries({ queryKey: reservationListsKey(propertyId) });
			void client.invalidateQueries({ queryKey: availabilityKey(propertyId) });
			// In place of this screen, so leaving the modal lands on the list, not back here.
			await goto(resolve(`/p/${propertyId}/reservations/${created.id}`), { replaceState: true });
		} catch (err) {
			createForm.failed(err);
			if (err instanceof ApiError && (err.status === 409 || err.status === 422)) {
				// Sold out meanwhile, or no longer sellable as quoted: back to the offers, as they are now.
				refused = err.message;
				selected = '';
				booking = offerRefused(booking);
				void availability.refetch();
				void focus(() => offersHeading);
			} else {
				createError = errorMessage(err);
			}
		}
	}

	function money(amount: number, currency: string): string {
		return `${currency} ${formatMoney(amount, currency)}`;
	}

	function occupancy(adults: number, children: number): string {
		const text = `${adults} adult${adults === 1 ? '' : 's'}`;
		if (children === 0) return text;
		return `${text}, ${children} ${children === 1 ? 'child' : 'children'}`;
	}

	function guestName(guest: Guest): string {
		return [guest.firstName, guest.lastName].filter(Boolean).join(' ');
	}

	/** `Resident` → `a resident`, for sentences. */
	function aResidency(residency: Residency): string {
		return `a ${residencyLabel(residency).toLowerCase()}`;
	}
</script>

<h1>New reservation</h1>

{#if me.isError || properties.isError}
	<p class="error" role="alert">{errorMessage(me.error ?? properties.error)}</p>
{:else if !me.data || !properties.data}
	<p>Loading…</p>
{:else if !manage}
	<p>You don't have permission to create reservations.</p>
{:else}
	<section class="step" aria-labelledby="stay-title">
		<h2 id="stay-title">1. Stay</h2>
		<form class="inline-form" aria-label="Stay" onsubmit={search}>
			<label>
				Check-in
				<input
					type="date"
					required
					min={businessDate}
					value={draft.checkIn}
					oninput={(event) =>
						changeStay(() => {
							draft.checkIn = event.currentTarget.value;
							if (draft.checkIn && draft.nights >= 1) {
								draft.checkOut = addDays(draft.checkIn, draft.nights);
							}
						})}
				/>
			</label>
			<label>
				Check-out
				<input
					type="date"
					required
					min={draft.checkIn ? addDays(draft.checkIn, 1) : businessDate}
					max={draft.checkIn ? addDays(draft.checkIn, MAX_NIGHTS) : undefined}
					value={draft.checkOut}
					oninput={(event) =>
						changeStay(() => {
							draft.checkOut = event.currentTarget.value;
							if (draft.checkIn && draft.checkOut) {
								draft.nights = nightsBetween(draft.checkIn, draft.checkOut);
							}
						})}
				/>
			</label>
			<label>
				Nights
				<input
					type="number"
					required
					min="1"
					max={MAX_NIGHTS}
					value={draft.nights}
					oninput={(event) =>
						changeStay(() => {
							draft.nights = event.currentTarget.valueAsNumber;
							if (draft.checkIn && draft.nights >= 1) {
								draft.checkOut = addDays(draft.checkIn, draft.nights);
							}
						})}
				/>
			</label>
			<label>
				Adults
				<input
					type="number"
					required
					min="1"
					max="50"
					value={draft.adults}
					oninput={(event) => changeStay(() => (draft.adults = event.currentTarget.valueAsNumber))}
				/>
			</label>
			<label>
				Children
				<input
					type="number"
					required
					min="0"
					max="50"
					value={draft.children}
					oninput={(event) =>
						changeStay(() => (draft.children = event.currentTarget.valueAsNumber))}
				/>
			</label>
			<fieldset>
				<legend>Residency</legend>
				{#each RESIDENCIES as residency (residency)}
					<label class="check">
						<input
							type="radio"
							name="residency"
							required
							value={residency}
							checked={draft.residency === residency}
							onchange={() => changeStay(() => (draft.residency = residency))}
						/>
						{residencyLabel(residency)}
					</label>
				{/each}
			</fieldset>
			<button>Search</button>
		</form>
	</section>

	{#if booking.stay}
		<section class="step" aria-labelledby="offers-title">
			<h2 id="offers-title" tabindex="-1" bind:this={offersHeading}>2. Offer</h2>
			<p class="hint">
				{formatStay(booking.stay.checkIn, booking.stay.checkOut)} · {occupancy(
					booking.stay.adults,
					booking.stay.children
				)} · {residencyLabel(booking.stay.residency)}
			</p>
			{#if refused}<p class="error" role="alert">{refused}</p>{/if}
			{#if availability.isError}
				<p class="error" role="alert">{errorMessage(availability.error)}</p>
				<button type="button" onclick={() => availability.refetch()}>Retry</button>
			{:else if !availability.data}
				<p>Loading…</p>
			{:else if offerGroups.length === 0}
				<p>No room types to offer.</p>
			{:else}
				<form aria-label="Offers" onsubmit={takeSelected}>
					{#each offerGroups as { type, rows } (type.roomTypeId)}
						<fieldset class="offers" disabled={type.free <= 0}>
							<legend>
								{type.code} · {type.name} · {type.free > 0 ? `${type.free} free` : 'Sold out'}
							</legend>
							{#each rows as row (offerKey(row))}
								<div class="offer">
									<label class="check">
										<input
											type="radio"
											name="offer"
											value={offerKey(row)}
											disabled={!row.sellable}
											checked={selected === offerKey(row)}
											onchange={() => selectOffer(row)}
											onclick={(event) => {
												// A pointer click takes the offer; arrow keys only move the choice.
												if (event.detail > 0) take(row);
											}}
										/>
										<span>{row.label}</span>
										<strong>{money(row.total, row.currency)}</strong>
									</label>
									{#if row.violations}<p class="hint">Can't be sold: {row.violations}</p>{/if}
									<details>
										<summary>Nightly prices</summary>
										<table aria-label="Nightly prices">
											<thead>
												<tr
													><th>Night</th><th class="number">Room</th><th class="number">Meal</th
													></tr
												>
											</thead>
											<tbody>
												{#each row.nights as night (night.date)}
													<tr>
														<td>{night.date}</td>
														<td class="number">{formatMoney(night.room, row.currency)}</td>
														<td class="number">{formatMoney(night.meal, row.currency)}</td>
													</tr>
												{/each}
											</tbody>
										</table>
									</details>
								</div>
							{:else}
								<p class="hint">No plan sells this room type for the stay.</p>
							{/each}
						</fieldset>
					{/each}
					<button disabled={!selected}>Continue</button>
				</form>
			{/if}
		</section>
	{/if}

	{#if booking.stay && booking.offer}
		<section class="step" aria-labelledby="guest-title">
			<h2 id="guest-title">3. Guest</h2>
			{#if booking.guest}
				<p>
					Booking for <strong>{guestName(booking.guest)}</strong> · {residencyLabel(
						booking.guest.residency
					)} · {idDocText(booking.guest)}
				</p>
			{/if}
			{#if booking.mismatch}
				<div class="notice" role="alert">
					<p>
						{guestName(booking.mismatch)} is {aResidency(booking.mismatch.residency)}, but the
						offers were searched for {aResidency(booking.stay.residency)}. Prices depend on
						residency, so search again to book for this guest.
					</p>
					<button type="button" onclick={searchAgainAsGuest}
						>Search again as {aResidency(booking.mismatch.residency)}</button
					>
				</div>
			{/if}
			<form class="inline-form" aria-label="Guest search" onsubmit={findGuestsNow}>
				<label>
					Find a guest
					<input
						type="search"
						placeholder="Name, email or phone"
						bind:this={guestSearch}
						bind:value={guestText}
						oninput={findGuests}
					/>
				</label>
				{#if !newGuest}
					<button type="button" class="secondary" onclick={openNewGuest}>New guest…</button>
				{/if}
			</form>
			{#if guestSearched && !newGuest}
				{#if guests.isError}
					<p class="error" role="alert">{errorMessage(guests.error)}</p>
				{:else if guests.data}
					<ul class="guests" aria-label="Guests found">
						{#each guests.data as guest (guest.id)}
							<li>
								<button type="button" class="secondary" onclick={() => choose(guest)}>
									<strong>{guestName(guest)}</strong>
									<span class="hint"
										>{[
											residencyLabel(guest.residency),
											guest.email,
											guest.phone,
											guest.idDocMasked ? idDocText(guest) : null
										]
											.filter(Boolean)
											.join(' · ')}</span
									>
								</button>
							</li>
						{:else}
							<li class="hint">No guest matches “{guestSearched}”.</li>
						{/each}
					</ul>
				{:else}
					<p>Searching…</p>
				{/if}
			{/if}
			{#if newGuest}
				<form class="form panel" aria-label="New guest" onsubmit={addGuest}>
					<label
						>First name <input
							maxlength="100"
							autocomplete="off"
							bind:this={newGuestFirst}
							bind:value={newGuest.firstName}
						/></label
					>
					<label
						>Last name <input
							required
							maxlength="100"
							autocomplete="off"
							bind:value={newGuest.lastName}
						/></label
					>
					<label
						>Email <input
							type="email"
							maxlength="254"
							autocomplete="off"
							bind:value={newGuest.email}
						/></label
					>
					<label
						>Phone <input
							type="tel"
							minlength="3"
							maxlength="30"
							autocomplete="off"
							bind:value={newGuest.phone}
						/></label
					>
					<label
						>Country <input
							pattern={'[A-Za-z]{2}'}
							title="Two letters, such as LK"
							autocomplete="off"
							bind:value={newGuest.country}
						/></label
					>
					<label>
						Guest residency
						<select bind:value={newGuest.residency}>
							<option value="resident">Resident</option>
							<option value="non_resident">Non-resident</option>
						</select>
					</label>
					<label>
						ID document
						<select bind:value={newGuest.idType}>
							<option value="">None</option>
							{#each ID_DOC_TYPES as type (type.value)}
								<option value={type.value}>{type.label}</option>
							{/each}
						</select>
					</label>
					<label
						>ID number <input
							required={!!newGuest.idType}
							disabled={!newGuest.idType}
							maxlength="50"
							autocomplete="off"
							bind:value={newGuest.idNumber}
						/></label
					>
					{#if guestError}<p class="error" role="alert">{guestError}</p>{/if}
					<div class="actions">
						<button disabled={pending.has('guest')}>Add guest</button>
						<button type="button" class="secondary" onclick={() => (newGuest = null)}>Cancel</button
						>
					</div>
				</form>
			{/if}
		</section>
	{/if}

	{#if booking.stay && booking.offer && booking.guest}
		{@const offer = booking.offer}
		<section class="step" aria-labelledby="review-title">
			<h2 id="review-title" tabindex="-1" bind:this={reviewHeading}>4. Review</h2>
			<form class="form" aria-label="Review" onsubmit={create}>
				<dl class="facts">
					<dt>Stay</dt>
					<dd>
						{formatStay(booking.stay.checkIn, booking.stay.checkOut)} · {occupancy(
							booking.stay.adults,
							booking.stay.children
						)} per room
					</dd>
					<dt>Room</dt>
					<dd>{offer.roomTypeCode} · {offer.roomTypeName} · {offer.label}</dd>
					<dt>Guest</dt>
					<dd>
						{guestName(booking.guest)} · {residencyLabel(booking.guest.residency)} · {idDocText(
							booking.guest
						)}
					</dd>
					<dt>Total</dt>
					<dd>
						<strong>{money(offer.total * rooms, offer.currency)}</strong>
						{#if rooms > 1}({rooms} rooms × {money(offer.total, offer.currency)}){/if}
					</dd>
				</dl>
				<label>
					Rooms
					<input type="number" required min="1" max={roomsAllowed(offer)} bind:value={rooms} />
				</label>
				<label>
					Source
					<select bind:value={source}>
						{#each SOURCES as value (value)}
							<option {value}>{sourceLabel(value)}</option>
						{/each}
					</select>
				</label>
				<label>Notes <textarea maxlength="2000" rows="3" bind:value={notes}></textarea></label>
				{#if createError}<p class="error" role="alert">{createError}</p>{/if}
				<button disabled={pending.has('create')}>Create reservation</button>
			</form>
		</section>
	{/if}
{/if}

<style>
	.step {
		margin-bottom: 1.5rem;
	}
	.step h2:focus {
		outline: none;
	}
	.step h2:focus-visible {
		outline: 2px solid var(--accent);
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.offers {
		margin: 0 0 var(--space);
		max-width: 40rem;
	}
	.offers:disabled {
		color: var(--muted);
	}
	.offer {
		padding: 0.25rem 0;
	}
	.offer .check {
		color: var(--text);
	}
	.offer .check strong {
		margin-left: auto;
	}
	.offer p,
	details {
		margin: 0.25rem 0 0 1.5rem;
	}
	.number {
		text-align: right;
	}
	.guests {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.25rem;
		max-width: 40rem;
	}
	.guests button {
		display: flex;
		gap: 0.5rem;
		width: 100%;
		text-align: left;
	}
	.panel {
		margin-top: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.notice {
		border: 1px solid var(--danger);
		border-radius: var(--radius);
		padding: 0 var(--space) var(--space);
		max-width: 40rem;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.25rem 1rem;
		margin: 0;
	}
	.facts dt {
		color: var(--muted);
	}
	.facts dd {
		margin: 0;
	}
</style>
