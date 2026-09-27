<!--
	One reservation, in a modal over the reservations table (the layout stays mounted beneath it). Closing it
	(Escape, the Close button or a click on the backdrop) goes back to the list with its filters: Back when
	the list is the previous entry, otherwise to the list URL, as after a deep link. Rooms are assigned,
	unassigned and cancelled here; cancelling shows its penalty before it is done.
-->
<script lang="ts">
	import { afterNavigate, goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { onMount } from 'svelte';
	import { SvelteMap } from 'svelte/reactivity';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { formatMoney } from '$lib/rates';
	import {
		describePenalty,
		describeTerms,
		fetchFreeRooms,
		fetchReservation,
		formatStay,
		freeRoomsKey,
		historyLabel,
		idDocText,
		offerLabel,
		reservationKey,
		reservationsKey,
		sourceLabel,
		statusLabel,
		type ReservationRoom
	} from '$lib/reservations';
	import { can, fetchMe } from '$lib/session';

	const LIST_ROUTE = '/(app)/p/[property]/reservations/(list)';
	const DETAIL_ROUTE = '/(app)/p/[property]/reservations/(list)/[id]';

	const propertyId = $derived(page.params.property ?? '');
	const id = $derived(page.params.id ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
	// The same key and fetcher as the table's prefetch on hover and focus, so an opened row is often
	// already loaded.
	const reservation = createQuery(() => ({
		queryKey: reservationKey(id),
		queryFn: ({ signal }) => fetchReservation(propertyId, id, signal)
	}));
	const data = $derived(reservation.data);
	const stay = $derived.by(() => {
		const rooms = data?.rooms ?? [];
		if (rooms.length === 0) return '';
		const arrival = rooms.map((room) => room.checkIn).reduce((a, b) => (b < a ? b : a));
		const departure = rooms.map((room) => room.checkOut).reduce((a, b) => (b > a ? b : a));
		return formatStay(arrival, departure);
	});

	let dialog = $state<HTMLDialogElement>();
	/** Whether the previous history entry is the list, so closing can go Back to it. */
	let fromList = false;

	afterNavigate(({ from }) => {
		fromList = from?.route.id === LIST_ROUTE && from.params?.property === propertyId;
	});
	onMount(() => dialog?.showModal());

	function closed() {
		// Already leaving (Back, or a link elsewhere): the navigation under way decides where to.
		if (page.route.id !== DETAIL_ROUTE) return;
		if (fromList) {
			history.back();
		} else {
			void goto(resolve(`/p/${propertyId}/reservations${page.url.search}`), {
				replaceState: true,
				noScroll: true
			});
		}
	}

	// Room actions, by reservation room id.
	const pending = new Pending();
	const problems = new SvelteMap<string, string>();
	const notices = new SvelteMap<string, string>();
	/** The room whose free-room picker is open, and the room chosen in it. */
	let picking = $state<string | null>(null);
	let choice = $state('');
	/** The room whose cancellation is waiting to be confirmed. */
	let confirming = $state<string | null>(null);

	const pickingRoom = $derived(data?.rooms.find((room) => room.id === picking));
	const free = createQuery(() => {
		const room = pickingRoom;
		return {
			queryKey: room
				? freeRoomsKey(propertyId, room.roomType.id, room.checkIn, room.checkOut)
				: freeRoomsKey(propertyId),
			queryFn: ({ signal }: { signal: AbortSignal }) =>
				fetchFreeRooms(propertyId, room!.roomType.id, room!.checkIn, room!.checkOut, signal),
			enabled: !!room
		};
	});
	// Keep the choice on a room the picker offers, e.g. after a refused room drops out of it.
	$effect(() => {
		const rooms = free.data;
		if (rooms && !rooms.some((room) => room.id === choice)) choice = rooms[0]?.id ?? '';
	});

	function openPicker(room: ReservationRoom) {
		confirming = null;
		problems.delete(room.id);
		notices.delete(room.id);
		choice = '';
		picking = room.id;
	}

	function confirmCancel(room: ReservationRoom) {
		picking = null;
		problems.delete(room.id);
		notices.delete(room.id);
		confirming = room.id;
	}

	/**
	 * Runs one room command. A 412 means the room changed since it was shown: the reservation is reloaded
	 * rather than retried with a stale If-Match. Any other refusal (a 409 names the reason) is shown by the
	 * room. The reservation, every list and the free rooms are refetched either way: the server's events do
	 * too, but the modal doesn't wait on them.
	 */
	async function command<T>(room: ReservationRoom, send: () => Promise<T>): Promise<T | null> {
		problems.delete(room.id);
		notices.delete(room.id);
		try {
			return await pending.run(room.id, send);
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				client.setQueryData(reservationKey(id), await fetchReservation(propertyId, id));
				problems.set(
					room.id,
					'Someone else changed this room. It now shows the latest version; check it and try again.'
				);
			} else {
				problems.set(room.id, errorMessage(err));
			}
			return null;
		} finally {
			await Promise.all([
				client.invalidateQueries({ queryKey: reservationKey(id) }),
				// Every list of the property, whatever its filter and sort.
				client.invalidateQueries({ queryKey: reservationsKey(propertyId).slice(0, 1) }),
				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
			]);
		}
	}

	async function assign(event: SubmitEvent, room: ReservationRoom) {
		event.preventDefault();
		const roomId = choice;
		const done = await command(room, async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/assign', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body: { room_id: roomId }
				})
			)
		);
		if (done) picking = null;
	}

	async function unassign(room: ReservationRoom) {
		await command(room, async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/unassign', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
	}

	async function cancel(room: ReservationRoom) {
		const done = await command(room, async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/cancel', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) }
				})
			)
		);
		confirming = null;
		if (done) {
			notices.set(
				room.id,
				done.penalty > 0
					? `Cancelled. The recorded penalty is ${money(done.penalty, done.currency)}.`
					: 'Cancelled at no cost.'
			);
		}
	}

	function money(amount: number, currency: string): string {
		return `${currency} ${formatMoney(amount, currency)}`;
	}

	function occupancy(room: ReservationRoom): string {
		const adults = `${room.adults} adult${room.adults === 1 ? '' : 's'}`;
		if (room.children === 0) return adults;
		return `${adults}, ${room.children} ${room.children === 1 ? 'child' : 'children'}`;
	}

	function when(at: string): string {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}
</script>

<!-- Clicking the backdrop closes the modal; from the keyboard, Escape does. -->
<dialog
	class="reservation"
	bind:this={dialog}
	aria-labelledby="reservation-title"
	onclose={closed}
	onclick={(event) => {
		if (event.target === dialog) dialog.close();
	}}
>
	<div class="content">
		<header>
			<h2 id="reservation-title">
				{data ? `${data.confirmationNo} · ${statusLabel(data.status)}` : 'Reservation'}
			</h2>
			<button type="button" class="secondary" onclick={() => dialog?.close()}>Close</button>
		</header>

		{#if reservation.isError && !data}
			<p class="error" role="alert">{errorMessage(reservation.error)}</p>
			<button type="button" onclick={() => reservation.refetch()}>Retry</button>
		{:else if data}
			<dl class="facts">
				<dt>Stay</dt>
				<dd>{stay}</dd>
				<dt>Rooms</dt>
				<dd>{data.rooms.length}</dd>
				<dt>Source</dt>
				<dd>{sourceLabel(data.source)}</dd>
				<dt>Booked</dt>
				<dd>{when(data.createdAt)}</dd>
				<dt>Total</dt>
				<dd>
					{data.totals.map((total) => money(total.amount, total.currency)).join(' + ') || '–'}
				</dd>
				{#if data.notes}
					<dt>Notes</dt>
					<dd>{data.notes}</dd>
				{/if}
			</dl>

			<h3>Booker</h3>
			<dl class="facts">
				<dt>Name</dt>
				<dd>{data.booker.firstName} {data.booker.lastName}</dd>
				<dt>Email</dt>
				<dd>{data.booker.email ?? '–'}</dd>
				<dt>Phone</dt>
				<dd>{data.booker.phone ?? '–'}</dd>
				<dt>Country</dt>
				<dd>{data.booker.country ?? '–'}</dd>
				<dt>ID</dt>
				<dd>{idDocText(data.booker)}</dd>
			</dl>

			{#each data.rooms as room (room.id)}
				<section class="room" aria-labelledby="room-{room.id}">
					<h3 id="room-{room.id}">{room.roomType.code} · {room.room?.number ?? 'Unassigned'}</h3>
					<dl class="facts">
						<dt>Dates</dt>
						<dd>{formatStay(room.checkIn, room.checkOut)}</dd>
						<dt>Occupancy</dt>
						<dd>{occupancy(room)}</dd>
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
							{describeTerms(room.cancellationTerms, room.currency)}{#if room.cancellationTerms}. A
								no-show costs {describePenalty(room.cancellationTerms.noShow, room.currency)}.{/if}
						</dd>
						{#if room.recordedPenalty !== null}
							<dt>Cancellation cost</dt>
							<dd>{money(room.recordedPenalty, room.currency)}</dd>
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

					{#if notices.get(room.id)}<p role="status">{notices.get(room.id)}</p>{/if}
					{#if problems.get(room.id)}<p class="error" role="alert">{problems.get(room.id)}</p>{/if}

					{#if manage}
						{#if picking === room.id}
							<form
								class="inline-form"
								aria-label="Assign a room"
								onsubmit={(e) => assign(e, room)}
							>
								{#if free.isError}
									<p class="error" role="alert">{errorMessage(free.error)}</p>
								{:else if free.data && free.data.length === 0}
									<p>No {room.roomType.code} room is free for these nights.</p>
								{:else}
									<label>
										Room
										<select required bind:value={choice} disabled={!free.data}>
											{#each free.data ?? [] as option (option.id)}
												<option value={option.id}
													>{option.number}{option.section ? ` · ${option.section}` : ''}</option
												>
											{/each}
										</select>
									</label>
									<button disabled={!choice || pending.has(room.id)}>Assign</button>
								{/if}
								<button type="button" class="secondary" onclick={() => (picking = null)}
									>Keep as is</button
								>
							</form>
						{:else if confirming === room.id}
							<div class="confirm">
								<p>
									{room.cancellationPenalty
										? `Cancelling now costs ${money(room.cancellationPenalty, room.currency)}.`
										: 'Cancelling now is free.'}
								</p>
								<div class="actions">
									<button disabled={pending.has(room.id)} onclick={() => cancel(room)}
										>Cancel this room</button
									>
									<button type="button" class="secondary" onclick={() => (confirming = null)}
										>Keep the room</button
									>
								</div>
							</div>
						{:else}
							<div class="actions">
								{#if room.status === 'CONFIRMED'}
									<button
										type="button"
										class="secondary"
										disabled={pending.has(room.id)}
										onclick={() => openPicker(room)}
										>{room.room ? 'Change room' : 'Assign room'}</button
									>
									{#if room.room}
										<button
											type="button"
											class="secondary"
											disabled={pending.has(room.id)}
											onclick={() => unassign(room)}>Unassign</button
										>
									{/if}
								{/if}
								{#if room.cancellationPenalty !== null}
									<button
										type="button"
										class="secondary"
										disabled={pending.has(room.id)}
										onclick={() => confirmCancel(room)}>Cancel room…</button
									>
								{/if}
							</div>
						{/if}
					{/if}
				</section>
			{/each}

			<h3>History</h3>
			<table aria-label="History">
				<thead>
					<tr><th>What</th><th>When</th><th>Who</th></tr>
				</thead>
				<tbody>
					{#each data.history as entry, index (index)}
						<tr>
							<td>{historyLabel(entry)}</td>
							<td>{when(entry.at)}</td>
							<td>{entry.actorName ?? 'A deleted user'}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p>Loading…</p>
		{/if}
	</div>
</dialog>

<style>
	.reservation {
		width: min(60rem, calc(100vw - 2rem));
		max-height: calc(100vh - 2rem);
		overflow: auto;
		padding: 0;
	}
	.reservation::backdrop {
		background: rgb(0 0 0 / 0.4);
	}
	.content {
		padding: 0 1.25rem 1.25rem;
	}
	header {
		position: sticky;
		top: 0;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space);
		padding: 1rem 0 0.5rem;
		background: var(--bg);
		border-bottom: 1px solid var(--border);
	}
	h2 {
		margin: 0;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.25rem 1rem;
		margin: var(--space) 0;
	}
	.facts dt {
		color: var(--muted);
	}
	.facts dd {
		margin: 0;
	}
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
</style>
