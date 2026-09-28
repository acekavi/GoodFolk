<!--
	One booked room of the reservation detail modal: its facts, nights and total, and every action on
	it (assign, unassign, cancel, modify, check in/undo/out, occupants). Self-contained: it runs its
	own commands and refreshes the reservation, every list and the free rooms afterwards, exactly as
	the modal's other actions do (`command`'s 412-reload pattern included).
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { addDays } from '$lib/inventory';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { formatMoney } from '$lib/rates';
	import {
		describePenalty,
		describeTerms,
		fetchAvailability,
		fetchFreeRooms,
		fetchReservation,
		findOffer,
		formatStay,
		freeRoomsKey,
		idDocText,
		modifyRoomBody,
		nightsReleasedOnCheckout,
		offerLabel,
		reservationKey,
		reservationListsKey,
		statusLabel,
		type Guest,
		type ModifyRoomDraft,
		type ReservationRoom
	} from '$lib/reservations';
	import type { RoomType } from '$lib/rooms';
	import GuestSearch from './GuestSearch.svelte';

	interface Props {
		propertyId: string;
		reservationId: string;
		room: ReservationRoom;
		/** Every room type of the property, for the modify form's type select and its codes. */
		roomTypes: RoomType[];
		businessDate: string;
		/** `manageReservations`: assign, cancel, modify and occupants. */
		manage: boolean;
		/** `frontDeskCheckIn`: check in, undo a same-day check-in, and check out. */
		checkInAllowed: boolean;
	}

	const {
		propertyId,
		reservationId,
		room,
		roomTypes,
		businessDate,
		manage,
		checkInAllowed
	}: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	let problem = $state('');
	let notice = $state('');

	// Only one panel is open at a time; opening one closes the others and clears the last result.
	let picking = $state(false);
	let choice = $state('');
	let confirmingCancel = $state(false);
	let modifying = $state(false);
	let confirmingCheckOut = $state(false);
	let addingOccupant = $state(false);

	function closePanels() {
		picking = false;
		confirmingCancel = false;
		modifying = false;
		confirmingCheckOut = false;
	}

	/**
	 * Runs one room command. A 412 means the room changed since it was shown: the reservation is
	 * reloaded rather than retried with a stale If-Match. Any other refusal (a 409 or 422 names the
	 * reason) is shown here. The reservation, every list and the free rooms are refetched either way.
	 */
	async function command<T>(key: string, send: () => Promise<T>): Promise<T | null> {
		problem = '';
		notice = '';
		try {
			return await pending.run(key, send);
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				client.setQueryData(
					reservationKey(reservationId),
					await fetchReservation(propertyId, reservationId)
				);
				problem =
					'Someone else changed this room. It now shows the latest version; check it and try again.';
			} else {
				problem = errorMessage(err);
			}
			return null;
		} finally {
			await Promise.all([
				client.invalidateQueries({ queryKey: reservationKey(reservationId) }),
				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) }),
				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
			]);
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

	function when(at: string): string {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}

	// Assign / unassign / cancel: the existing picker and confirm-before-cancel pattern.
	function openPicker() {
		closePanels();
		problem = '';
		notice = '';
		choice = '';
		picking = true;
	}

	function openCancelConfirm() {
		closePanels();
		problem = '';
		notice = '';
		confirmingCancel = true;
	}

	const free = createQuery(() => ({
		queryKey: picking
			? freeRoomsKey(propertyId, room.roomType.id, room.checkIn, room.checkOut)
			: freeRoomsKey(propertyId),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchFreeRooms(propertyId, room.roomType.id, room.checkIn, room.checkOut, signal),
		enabled: picking
	}));
	// Never leave the choice on a room the picker no longer offers, e.g. after a refused room drops
	// out of it: back to no choice, so Assign never silently books another room.
	$effect(() => {
		const rooms = free.data;
		if (rooms && choice && !rooms.some((r) => r.id === choice)) choice = '';
	});

	async function assign(event: SubmitEvent) {
		event.preventDefault();
		const roomId = choice;
		const done = await command('assign', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/assign', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body: { room_id: roomId }
				})
			)
		);
		if (done) picking = false;
	}

	async function unassign() {
		await command('unassign', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/unassign', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function cancel() {
		const done = await command('cancel', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/cancel', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
		confirmingCancel = false;
		if (done) {
			notice =
				done.penalty > 0
					? `Cancelled. The recorded penalty is ${money(done.penalty, done.currency)}.`
					: 'Cancelled at no cost.';
		}
	}

	// Modify: dates, type and occupancy for a confirmed room; only the departure for a checked-in one.
	const activeRoomTypes = $derived(
		roomTypes.filter((type) => type.active || type.id === room.roomType.id)
	);
	const roomTypeCode = (id: string) => roomTypes.find((type) => type.id === id)?.code ?? '?';

	function draftFromRoom(): ModifyRoomDraft {
		return {
			checkIn: room.checkIn,
			checkOut: room.checkOut,
			roomTypeId: room.roomType.id,
			adults: room.adults,
			children: room.children,
			keepPrice: false,
			reprice: false
		};
	}

	let draft = $state(draftFromRoom());
	let previewOpen = $state(false);

	function openModify() {
		closePanels();
		problem = '';
		notice = '';
		draft = draftFromRoom();
		previewOpen = false;
		modifying = true;
	}

	function changeDraft(change: () => void) {
		change();
		previewOpen = false;
	}

	const preview = createQuery(() => ({
		queryKey: [
			'reservationRoomPreview',
			propertyId,
			draft.checkIn,
			draft.checkOut,
			draft.adults,
			draft.children,
			room.primaryGuest.residency
		],
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchAvailability(
				propertyId,
				draft.checkIn,
				draft.checkOut,
				draft.adults,
				draft.children,
				room.primaryGuest.residency,
				signal
			),
		enabled: previewOpen
	}));
	const previewOffer = $derived(
		preview.data
			? findOffer(preview.data, draft.roomTypeId, room.ratePlan.id, room.mealPlan)
			: undefined
	);

	async function saveModify(event: SubmitEvent) {
		event.preventDefault();
		const oldNumber = room.room?.number;
		const body = modifyRoomBody(
			{
				checkIn: room.checkIn,
				checkOut: room.checkOut,
				roomTypeId: room.roomType.id,
				adults: room.adults,
				children: room.children
			},
			draft
		);
		const result = await command('modify', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/modify', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body
				})
			)
		);
		if (result) {
			modifying = false;
			previewOpen = false;
			if (result.unassigned) {
				notice = `Room ${oldNumber ?? '?'} was unassigned because it is not a ${roomTypeCode(result.room_type_id)}.`;
			}
		}
	}

	// Check in / undo / check out.
	const released = $derived(nightsReleasedOnCheckout(room, businessDate));
	const checkOutMessage = $derived(
		released.length > 0
			? `Checking out today releases ${released.length} night${released.length === 1 ? '' : 's'} (${released.join(', ')}).`
			: 'Checking out now keeps the full stay.'
	);

	async function checkIn() {
		await command('checkIn', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/check-in', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function undoCheckIn() {
		await command('undoCheckIn', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/undo-check-in', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function checkOut() {
		const done = await command('checkOut', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/check-out', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
		confirmingCheckOut = false;
		if (done) {
			notice =
				done.released_nights.length > 0
					? `Checked out. Released ${done.released_nights.length} night${done.released_nights.length === 1 ? '' : 's'} (${done.released_nights.join(', ')}).`
					: 'Checked out.';
		}
	}

	// Occupants.
	const CLOSED_STATUSES = ['CANCELLED', 'NO_SHOW', 'CHECKED_OUT'];
	const occupantsAllowed = $derived(!CLOSED_STATUSES.includes(room.status));
	const roomType = $derived(roomTypes.find((type) => type.id === room.roomType.id));
	const maxExtraOccupants = $derived(roomType ? Math.max(0, roomType.maxOccupancy - 1) : Infinity);
	const excludedGuestIds = $derived([room.primaryGuest.id, ...room.occupants.map((o) => o.id)]);

	function openAddOccupant() {
		problem = '';
		notice = '';
		addingOccupant = true;
	}

	async function addOccupant(guest: Guest) {
		const done = await command('addOccupant', async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/guests', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body: { guest_id: guest.id }
				})
			)
		);
		if (done) addingOccupant = false;
	}

	async function removeOccupant(guestId: string) {
		await command(`removeOccupant:${guestId}`, async () =>
			unwrap(
				await rest.DELETE('/api/v1/properties/{property}/reservation-rooms/{room}/guests/{guest}', {
					params: {
						path: { property: propertyId, room: room.id, guest: guestId },
						header: ifMatch(room.version)
					}
				})
			)
		);
	}
</script>

<section class="room" aria-labelledby="room-{room.id}">
	<h3 id="room-{room.id}">{room.roomType.code} · {room.room?.number ?? 'Unassigned'}</h3>
	<dl class="facts">
		<dt>Dates</dt>
		<dd>{formatStay(room.checkIn, room.checkOut)}</dd>
		<dt>Occupancy</dt>
		<dd>{occupancy(room.adults, room.children)}</dd>
		<dt>Plan</dt>
		<dd>{offerLabel({ ratePlanCode: room.ratePlan.code, mealPlan: room.mealPlan })}</dd>
		<dt>Status</dt>
		<dd>{statusLabel(room.status)}</dd>
		<dt>Guest</dt>
		<dd>
			{room.primaryGuest.firstName}
			{room.primaryGuest.lastName} · {idDocText(room.primaryGuest)}
		</dd>
		<dt>Cancellation</dt>
		<dd>
			{describeTerms(room.cancellationTerms, room.currency)}{#if room.cancellationTerms}. A no-show
				costs {describePenalty(room.cancellationTerms.noShow, room.currency)}.{/if}
		</dd>
		{#if room.recordedPenalty !== null}
			<dt>Cancellation cost</dt>
			<dd>{money(room.recordedPenalty, room.currency)}</dd>
		{/if}
		{#if room.checkedInAt}
			<dt>Checked in</dt>
			<dd>{when(room.checkedInAt)}</dd>
		{/if}
		{#if room.checkedOutAt}
			<dt>Checked out</dt>
			<dd>{when(room.checkedOutAt)}</dd>
		{/if}
	</dl>

	<table aria-label="Nights">
		<thead>
			<tr><th>Date</th><th class="number">Room</th><th class="number">Meal</th></tr>
		</thead>
		<tbody>
			{#each room.nights as night (night.date)}
				<tr>
					<td>{night.date}</td>
					<td class="number">{formatMoney(night.room, room.currency)}</td>
					<td class="number">{formatMoney(night.meal, room.currency)}</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<p>Total <strong>{money(room.total, room.currency)}</strong></p>

	{#if notice}<p role="status">{notice}</p>{/if}
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}

	{#if picking}
		<form class="inline-form" aria-label="Assign a room" onsubmit={assign}>
			{#if free.isError}
				<p class="error" role="alert">{errorMessage(free.error)}</p>
			{:else if free.data && free.data.length === 0}
				<p>No {room.roomType.code} room is free for these nights.</p>
			{:else}
				<label>
					Room
					<select required bind:value={choice} disabled={!free.data}>
						<option value="" disabled>Choose a room</option>
						{#each free.data ?? [] as option (option.id)}
							<option value={option.id}
								>{option.number}{option.section ? ` · ${option.section}` : ''}</option
							>
						{/each}
					</select>
				</label>
				<button disabled={!choice || pending.has('assign')}>Assign</button>
			{/if}
			<button type="button" class="secondary" onclick={() => (picking = false)}>Keep as is</button>
		</form>
	{:else if confirmingCancel}
		<div class="confirm">
			<p>
				{room.cancellationPenalty
					? `Cancelling now costs ${money(room.cancellationPenalty, room.currency)}.`
					: 'Cancelling now is free.'}
			</p>
			<div class="actions">
				<button disabled={pending.has('cancel')} onclick={cancel}>Cancel this room</button>
				<button type="button" class="secondary" onclick={() => (confirmingCancel = false)}
					>Keep the room</button
				>
			</div>
		</div>
	{:else if modifying}
		{#if room.status === 'CHECKED_IN'}
			<form class="inline-form" aria-label="Change departure" onsubmit={saveModify}>
				<label>
					Check-out
					<input
						type="date"
						required
						min={addDays(room.checkIn, 1)}
						value={draft.checkOut}
						oninput={(event) => changeDraft(() => (draft.checkOut = event.currentTarget.value))}
					/>
				</label>
				{#if problem}<p class="error" role="alert">{problem}</p>{/if}
				<button disabled={pending.has('modify')}>Save</button>
				<button type="button" class="secondary" onclick={() => (modifying = false)}>Cancel</button>
			</form>
		{:else}
			<form class="form panel" aria-label="Modify room" onsubmit={saveModify}>
				<label>
					Check-in
					<input
						type="date"
						required
						value={draft.checkIn}
						oninput={(event) => changeDraft(() => (draft.checkIn = event.currentTarget.value))}
					/>
				</label>
				<label>
					Check-out
					<input
						type="date"
						required
						min={addDays(draft.checkIn, 1)}
						value={draft.checkOut}
						oninput={(event) => changeDraft(() => (draft.checkOut = event.currentTarget.value))}
					/>
				</label>
				<label>
					Room type
					<select
						value={draft.roomTypeId}
						onchange={(event) => changeDraft(() => (draft.roomTypeId = event.currentTarget.value))}
					>
						{#each activeRoomTypes as type (type.id)}
							<option value={type.id}>{type.code} · {type.name}</option>
						{/each}
					</select>
				</label>
				<label>
					Adults
					<input
						type="number"
						required
						min="1"
						max="50"
						value={draft.adults}
						oninput={(event) =>
							changeDraft(() => (draft.adults = event.currentTarget.valueAsNumber))}
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
							changeDraft(() => (draft.children = event.currentTarget.valueAsNumber))}
					/>
				</label>
				<label class="check">
					<input
						type="checkbox"
						bind:checked={draft.keepPrice}
						onchange={() => (previewOpen = false)}
					/>
					Keep the booked price (upgrade)
				</label>
				<label class="check">
					<input
						type="checkbox"
						bind:checked={draft.reprice}
						onchange={() => (previewOpen = false)}
					/>
					Reprice every night
				</label>

				{#if previewOpen}
					{#if preview.isError}
						<p class="error" role="alert">{errorMessage(preview.error)}</p>
					{:else if !preview.data}
						<p>Pricing…</p>
					{:else if previewOffer}
						<p>
							New total <strong>{money(previewOffer.total, previewOffer.currency)}</strong>
							{#if draft.keepPrice}
								<span class="hint"
									>(every night at the new stay's price; nights kept at their booked price will cost
									less)</span
								>
							{/if}
						</p>
						<table aria-label="New nightly prices">
							<thead>
								<tr><th>Date</th><th class="number">Room</th><th class="number">Meal</th></tr>
							</thead>
							<tbody>
								{#each previewOffer.nights as night (night.date)}
									<tr>
										<td>{night.date}</td>
										<td class="number">{formatMoney(night.room, previewOffer.currency)}</td>
										<td class="number">{formatMoney(night.meal, previewOffer.currency)}</td>
									</tr>
								{/each}
							</tbody>
						</table>
					{:else}
						<p>
							No {roomTypeCode(draft.roomTypeId)} offer sells this stay on {room.ratePlan.code}.
						</p>
					{/if}
				{/if}

				{#if problem}<p class="error" role="alert">{problem}</p>{/if}
				<div class="actions">
					<button type="button" class="secondary" onclick={() => (previewOpen = true)}
						>Preview</button
					>
					<button disabled={pending.has('modify')}>Save</button>
					<button type="button" class="secondary" onclick={() => (modifying = false)}>Cancel</button
					>
				</div>
			</form>
		{/if}
	{:else if confirmingCheckOut}
		<div class="confirm">
			<p>{checkOutMessage}</p>
			<div class="actions">
				<button disabled={pending.has('checkOut')} onclick={checkOut}>Check out</button>
				<button type="button" class="secondary" onclick={() => (confirmingCheckOut = false)}
					>Stay checked in</button
				>
			</div>
		</div>
	{:else}
		<div class="actions">
			{#if manage}
				{#if room.status === 'CONFIRMED'}
					<button
						type="button"
						class="secondary"
						disabled={pending.has('assign')}
						onclick={openPicker}>{room.room ? 'Change room' : 'Assign room'}</button
					>
					{#if room.room}
						<button
							type="button"
							class="secondary"
							disabled={pending.has('unassign')}
							onclick={unassign}>Unassign</button
						>
					{/if}
				{/if}
				{#if room.status === 'CONFIRMED' || room.status === 'CHECKED_IN'}
					<button type="button" class="secondary" onclick={openModify}
						>{room.status === 'CHECKED_IN' ? 'Change departure…' : 'Modify…'}</button
					>
				{/if}
				{#if room.cancellationPenalty !== null}
					<button
						type="button"
						class="secondary"
						disabled={pending.has('cancel')}
						onclick={openCancelConfirm}>Cancel room…</button
					>
				{/if}
			{/if}
			{#if checkInAllowed}
				{#if room.canCheckIn}
					<button disabled={pending.has('checkIn')} onclick={checkIn}>Check in</button>
				{/if}
				{#if room.canUndoCheckIn}
					<button class="secondary" disabled={pending.has('undoCheckIn')} onclick={undoCheckIn}
						>Undo check-in</button
					>
				{/if}
				{#if room.canCheckOut}
					<button
						type="button"
						class="secondary"
						onclick={() => {
							closePanels();
							problem = '';
							notice = '';
							confirmingCheckOut = true;
						}}>Check out…</button
					>
				{/if}
			{/if}
		</div>
	{/if}

	{#if manage}
		<h4>Occupants</h4>
		{#if room.occupants.length === 0}
			<p class="hint">No additional occupants.</p>
		{:else}
			<ul class="occupants">
				{#each room.occupants as occupant (occupant.id)}
					<li>
						{occupant.firstName}
						{occupant.lastName} · {idDocText(occupant)}
						{#if occupantsAllowed}
							<button
								type="button"
								class="secondary"
								disabled={pending.has(`removeOccupant:${occupant.id}`)}
								onclick={() => removeOccupant(occupant.id)}>Remove</button
							>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if occupantsAllowed}
			{#if addingOccupant}
				<GuestSearch
					{propertyId}
					exclude={excludedGuestIds}
					onChoose={(guest) => void addOccupant(guest)}
				/>
				<button type="button" class="secondary" onclick={() => (addingOccupant = false)}
					>Cancel</button
				>
			{:else}
				<button
					type="button"
					class="secondary"
					disabled={room.occupants.length >= maxExtraOccupants}
					onclick={openAddOccupant}>Add occupant…</button
				>
			{/if}
		{/if}
	{/if}
</section>

<style>
	.room {
		margin: var(--space) 0;
		padding: 0 var(--space) var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.number {
		text-align: right;
	}
	.confirm {
		display: grid;
		gap: 0.5rem;
	}
	.panel {
		margin-top: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.check {
		display: flex;
		flex-direction: row;
		gap: 0.4rem;
		align-items: center;
	}
	h4 {
		margin-bottom: 0.25rem;
	}
	.occupants {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.25rem;
		margin: 0 0 var(--space);
	}
	.occupants li {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
</style>
