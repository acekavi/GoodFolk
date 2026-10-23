<!--
	Confirms one row action of the night audit's review, showing its fee or price effect first: a no-show (its
	penalty), a delayed arrival or an extra night (the stay's new total or the night's price, read from the
	reservation), or a check-out (the guest folio's balances). Each is sent with `If-Match` on the reservation room.
	A 412 means the stay changed since the review was read: the dialog closes and `ondone` reports it, so the page
	refetches. Any other refusal shows the server's message here.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { money } from '$lib/folio';
	import { addDays } from '$lib/inventory';
	import {
		lastNightAmount,
		nightAuditKey,
		owedText,
		totalAfterDelay,
		type AuditAction
	} from '$lib/night-audit';
	import {
		fetchReservation,
		formatStay,
		reservationKey,
		reservationListsKey
	} from '$lib/reservations';

	interface Props {
		propertyId: string;
		businessDate: string;
		/** The action to confirm; `null` closes the dialog. */
		action: AuditAction | null;
		/** After the dialog sent the action: what it did, or why the review must be read again. */
		ondone: (result: { notice?: string; problem?: string }) => void;
	}

	let { propertyId, businessDate, action = $bindable(), ondone }: Props = $props();

	const client = useQueryClient();
	let dialog = $state<HTMLDialogElement>();
	let problem = $state('');
	let busy = $state(false);

	const TITLES = {
		noShow: 'Mark as a no-show',
		delay: 'Delay arrival',
		checkOut: 'Check out',
		extend: 'Extend a night'
	} as const;
	const SUBMIT = {
		noShow: 'Mark no-show',
		delay: 'Delay arrival',
		checkOut: 'Check out',
		extend: 'Extend a night'
	} as const;

	// A delay or an extension prices from the stay's nights, which only the reservation carries.
	const priced = $derived(action?.kind === 'delay' || action?.kind === 'extend');
	const reservationId = $derived(action?.row.reservationId ?? '');
	const reservation = createQuery(() => ({
		queryKey: reservationKey(reservationId),
		queryFn: ({ signal }) => fetchReservation(propertyId, reservationId, signal),
		enabled: priced && !!reservationId
	}));
	const room = $derived(
		reservation.data?.rooms.find((entry) => entry.id === action?.row.reservationRoomId)
	);
	const tomorrow = $derived(addDays(businessDate, 1));

	$effect(() => {
		if (!dialog) return;
		if (action && !dialog.open) {
			problem = '';
			dialog.showModal();
		} else if (!action && dialog.open) {
			dialog.close();
		}
	});

	async function send(current: AuditAction): Promise<string> {
		const params = {
			path: { property: propertyId, room: current.row.reservationRoomId },
			header: ifMatch(current.row.version)
		};
		switch (current.kind) {
			case 'noShow': {
				const done = unwrap(
					await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/no-show', {
						params
					})
				);
				return done.penalty > 0
					? `Marked a no-show. The fee of ${money(done.penalty, done.currency)} is posted.`
					: 'Marked a no-show at no cost.';
			}
			case 'delay': {
				const done = unwrap(
					await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/delay-arrival', {
						params
					})
				);
				return `Arrival delayed: the stay is now ${formatStay(done.check_in, done.check_out)}.`;
			}
			case 'checkOut': {
				unwrap(
					await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/check-out', {
						params
					})
				);
				return 'Checked out.';
			}
			case 'extend': {
				const done = unwrap(
					await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/extend', {
						params
					})
				);
				return `Extended a night: the guest now leaves on ${done.check_out}.`;
			}
		}
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!action) return;
		const current = action;
		busy = true;
		problem = '';
		try {
			const notice = await send(current);
			action = null;
			ondone({ notice });
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				action = null;
				ondone({ problem: 'This booking changed; review it again' });
			} else {
				problem = errorMessage(err);
			}
		} finally {
			busy = false;
			await Promise.all([
				client.invalidateQueries({ queryKey: nightAuditKey(propertyId) }),
				client.invalidateQueries({ queryKey: reservationKey(current.row.reservationId) }),
				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) })
			]);
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="audit-action-title"
	onclose={() => (action = null)}
>
	{#if action}
		<form class="form" onsubmit={submit}>
			<h2 id="audit-action-title">{TITLES[action.kind]}</h2>
			<p>
				{action.row.guest} · {action.row.confirmation}{action.row.roomNumber
					? ` · Room ${action.row.roomNumber}`
					: ''}
			</p>
			{#if action.kind === 'noShow'}
				<p>The room is released from {businessDate} on.</p>
				<p>
					{action.row.noShowPenalty > 0
						? `A no-show fee of ${money(action.row.noShowPenalty, action.row.currency)} is posted to the reservation's deposit folio.`
						: 'There is no no-show fee under its booked terms.'}
				</p>
			{:else if action.kind === 'delay'}
				<p>
					The stay becomes {formatStay(tomorrow, action.row.departure)}; the nights before {tomorrow}
					are released.
				</p>
				{#if room}
					<p>
						Its total becomes {money(totalAfterDelay(room.nights, tomorrow), room.currency)} (now {money(
							room.total,
							room.currency
						)}).
					</p>
				{:else if reservation.error}
					<p class="error" role="alert">{errorMessage(reservation.error)}</p>
				{/if}
			{:else if action.kind === 'checkOut'}
				{@const owed = owedText(action.row.balances)}
				{#if owed === null}
					<p>The guest folio is settled.</p>
				{:else}
					<p>The guest folio still has {owed}; it stays open after check-out.</p>
				{/if}
			{:else if action.kind === 'extend'}
				<p>The guest stays tonight ({businessDate}) and leaves on {tomorrow}.</p>
				{#if room}
					{@const price = lastNightAmount(room.nights)}
					{#if price !== null}
						<p>The night costs what the last one did: {money(price, room.currency)}.</p>
					{/if}
				{:else if reservation.error}
					<p class="error" role="alert">{errorMessage(reservation.error)}</p>
				{/if}
			{/if}
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<div class="actions">
				<button disabled={busy}>{SUBMIT[action.kind]}</button>
				<button type="button" class="secondary" onclick={() => (action = null)}>Back</button>
			</div>
		</form>
	{/if}
</dialog>
