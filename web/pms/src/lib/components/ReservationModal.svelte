<!--
	One reservation, in a modal dialog. The reservations table opens it over itself (the `[id]` route) and the
	tape chart over the chart (shallow routing); each decides what closing means and passes it as `onclose`,
	called when the dialog closes (Escape, the Close button or a click on the backdrop). Every room's own
	actions (assign, cancel, modify, check in/out, occupants) live in `RoomCard`; this component keeps the
	reservation-wide facts, the booker, the billed account and the history.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { onMount } from 'svelte';
	import { accountKindLabel, accountsKey, fetchAccounts } from '$lib/accounts';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import RoomCard from '$lib/components/RoomCard.svelte';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { formatMoney } from '$lib/rates';
	import {
		fetchReservation,
		formatStay,
		historyLabel,
		idDocText,
		reservationKey,
		reservationListsKey,
		sourceLabel,
		statusLabel
	} from '$lib/reservations';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	interface Props {
		propertyId: string;
		id: string;
		onclose: () => void;
	}

	let { propertyId, id, onclose }: Props = $props();

	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
	const checkInAllowed = $derived(!!me.data && can(me.data, 'frontDeskCheckIn', propertyId));
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

	// Every room type of the property (RoomCard's modify form) and the business date (its check-out
	// confirmation), read once here rather than by every room card.
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }: { signal: AbortSignal }) => fetchRoomTypes(propertyId, signal)
	}));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }: { signal: AbortSignal }) => fetchProperties(signal)
	}));
	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);

	let dialog = $state<HTMLDialogElement>();
	onMount(() => dialog?.showModal());

	function money(amount: number, currency: string): string {
		return `${currency} ${formatMoney(amount, currency)}`;
	}

	function when(at: string): string {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}

	// The billed account: a select of active accounts, PATCHed with If-Match on the reservation.
	const accounts = createQuery(() => ({
		queryKey: accountsKey(propertyId),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchAccounts(propertyId, undefined, false, signal),
		enabled: manage
	}));
	const accountPending = new Pending();
	let accountProblem = $state('');

	async function setAccount(accountId: string | null) {
		if (!data) return;
		accountProblem = '';
		try {
			await accountPending.run('account', async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/reservations/{reservation}', {
						params: {
							path: { property: propertyId, reservation: id },
							header: ifMatch(data.version)
						},
						body: { account_id: accountId }
					})
				)
			);
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				client.setQueryData(reservationKey(id), await fetchReservation(propertyId, id));
				accountProblem =
					'Someone else changed this reservation. It now shows the latest version; check it and try again.';
			} else {
				accountProblem = errorMessage(err);
			}
		} finally {
			await Promise.all([
				client.invalidateQueries({ queryKey: reservationKey(id) }),
				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) })
			]);
		}
	}
</script>

<!-- Clicking the backdrop closes the modal; from the keyboard, Escape does. -->
<dialog
	class="reservation"
	bind:this={dialog}
	aria-labelledby="reservation-title"
	{onclose}
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
				<dt>Account</dt>
				<dd>
					{#if manage}
						<select
							aria-label="Billed account"
							value={data.account?.id ?? ''}
							disabled={accountPending.has('account')}
							onchange={(event) => setAccount(event.currentTarget.value || null)}
						>
							<option value="">No account</option>
							{#each accounts.data ?? [] as account (account.id)}
								<option value={account.id}>{account.name} · {accountKindLabel(account.kind)}</option
								>
							{/each}
						</select>
					{:else}
						{data.account ? `${data.account.name} · ${accountKindLabel(data.account.kind)}` : '–'}
					{/if}
				</dd>
				{#if data.notes}
					<dt>Notes</dt>
					<dd>{data.notes}</dd>
				{/if}
			</dl>
			{#if accountProblem}<p class="error" role="alert">{accountProblem}</p>{/if}

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
				<RoomCard
					{propertyId}
					reservationId={id}
					{room}
					roomTypes={roomTypes.data ?? []}
					{businessDate}
					{manage}
					{checkInAllowed}
				/>
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
</style>
