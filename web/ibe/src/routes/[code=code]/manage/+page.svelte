<script lang="ts">
	import { afterNavigate, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount, tick } from 'svelte';
	import { NETWORK_TROUBLE, publicApi, TOO_MANY_TRIES } from '$lib/api';
	import Contact from '$lib/components/Contact.svelte';
	import {
		dateText,
		guestsText,
		mealPlanText,
		money,
		nights,
		plural,
		totalsText
	} from '$lib/format';

	let { data } = $props();

	const site = $derived(data.site);
	/** The link's token, kept in memory for the cancel once the address no longer shows it. */
	const token = page.url.searchParams.get('token') ?? '';

	// svelte-ignore state_referenced_locally
	let managed = $state(data.managed);
	let confirming = $state(false);
	let busy = $state(false);
	let message: string | null = $state(null);
	let justCancelled = $state(false);
	let cancelButton: HTMLButtonElement | undefined = $state();
	/** The page paints before it hydrates: until then a tap on Cancel would do nothing, so the button waits. */
	let hydrated = $state(false);

	/** Moves focus to what the guest's last action brought up, so it is not lost with the button that went. */
	function focusNow(node: HTMLElement) {
		node.focus();
	}

	async function keep() {
		confirming = false;
		await tick();
		cancelButton?.focus();
	}

	onMount(() => {
		hydrated = true;
	});

	// The token leaves the address bar and the history entry: the page has what it needs. After the router has
	// started (`afterNavigate` runs once it has, also for the first load), as `replaceState` needs it.
	afterNavigate(() => {
		if (!page.url.searchParams.has('token')) return;
		const clean = new URL(page.url);
		clean.searchParams.delete('token');
		replaceState(clean, page.state);
	});

	const STATUS: Record<string, string> = {
		tentative: 'Booked',
		confirmed: 'Confirmed',
		checked_in: 'Checked in',
		checked_out: 'Checked out',
		cancelled: 'Cancelled',
		no_show: 'No-show'
	};

	function hasAmounts(amounts: Record<string, number>) {
		return Object.values(amounts).some((amount) => amount > 0);
	}

	async function cancel() {
		busy = true;
		message = null;
		try {
			const {
				data: view,
				error,
				response
			} = await publicApi().POST('/api/public/v1/ibe/{code}/bookings/manage/cancel', {
				params: { path: { code: site.code } },
				body: { token }
			});
			if (view) {
				managed = { kind: 'booking', view };
				justCancelled = true;
				confirming = false;
			} else if (response.status === 410) {
				managed = { kind: 'gone' };
			} else if (response.status === 429) {
				message = TOO_MANY_TRIES;
			} else {
				message =
					(error as { detail?: string } | undefined)?.detail ??
					'The booking could not be cancelled. Try again in a moment.';
				confirming = false;
			}
		} catch {
			message = NETWORK_TROUBLE;
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>Your booking | {site.name}</title>
	<meta name="robots" content="noindex" />
	<meta name="referrer" content="no-referrer" />
</svelte:head>

<main class="wrap manage">
	{#if managed.kind === 'booking'}
		{@const view = managed.view}
		<h1>Your booking</h1>
		{#if justCancelled}
			<p class="notice" role="status" tabindex="-1" use:focusNow>
				Your booking is cancelled. We have sent you an email to confirm it.
			</p>
		{/if}
		<section class="summary" aria-labelledby="booking-heading">
			<h2 id="booking-heading">{view.property_name}</h2>
			<dl>
				<div>
					<dt>Confirmation number</dt>
					<dd>{view.confirmation_no}</dd>
				</div>
				<div>
					<dt>Status</dt>
					<dd>{STATUS[view.status] ?? view.status}</dd>
				</div>
				<div>
					<dt>Dates</dt>
					<dd>
						{dateText(view.check_in)} to {dateText(view.check_out)},
						{plural(nights(view.check_in, view.check_out), 'night')}
					</dd>
				</div>
			</dl>
			<ol class="rooms">
				{#each view.rooms as room, index (index)}
					<li>
						<p><strong>{room.room_type}</strong></p>
						<p class="muted">
							{room.plan}, {mealPlanText(room.meal_plan)}, {guestsText(room.adults, room.children)}
						</p>
						<p>{money(room.total, room.currency)}</p>
					</li>
				{/each}
			</ol>
			<p class="total">Total {totalsText(view.totals)}</p>
			<p class="muted">Taxes and service charge included. You pay at the hotel.</p>
			{#if hasAmounts(view.cancellation_penalty)}
				<p>Cancellation charge: {totalsText(view.cancellation_penalty)}</p>
			{/if}
			<h3>Cancellation</h3>
			{#each view.cancellation as text (text)}<p class="muted">{text}</p>{/each}
		</section>

		{#if view.cancellable}
			<section class="cancel" aria-labelledby="cancel-heading">
				<h2 id="cancel-heading">Cancel this booking</h2>
				<p>
					{#if hasAmounts(view.penalty_if_cancelled)}
						Cancelling now costs <strong>{totalsText(view.penalty_if_cancelled)}</strong>, under the
						terms you booked with.
					{:else}
						Cancelling now is free.
					{/if}
				</p>
				{#if confirming}
					<div class="confirm" role="group" aria-label="Confirm the cancellation">
						<p><strong>Cancel the whole booking, every room?</strong></p>
						<div class="actions">
							<button
								type="button"
								class="button danger"
								disabled={busy}
								onclick={cancel}
								use:focusNow
							>
								{busy ? 'Cancelling…' : 'Yes, cancel the booking'}
							</button>
							<button type="button" class="button quiet" disabled={busy} onclick={keep}
								>Keep the booking</button
							>
						</div>
					</div>
				{:else}
					<button
						type="button"
						class="button quiet"
						bind:this={cancelButton}
						disabled={!hydrated}
						onclick={() => (confirming = true)}
					>
						Cancel booking
					</button>
				{/if}
			</section>
		{:else if view.status !== 'cancelled'}
			<p class="muted">This booking can no longer be cancelled online.</p>
		{/if}

		{#key message}
			{#if message}
				<p class="notice error" role="alert" tabindex="-1" use:focusNow>{message}</p>
			{/if}
		{/key}

		<Contact
			lead="To change your booking, contact the hotel."
			phone={view.contact_phone}
			email={view.contact_email}
		/>
	{:else if managed.kind === 'gone'}
		<h1>This link has expired or is not valid</h1>
		<p>
			Manage links work until 30 days after check-out. Check the link in your confirmation email.
		</p>
		<Contact
			lead="For anything about your booking, contact the hotel."
			phone={site.contact_phone}
			email={site.contact_email}
		/>
	{:else if managed.kind === 'too_many'}
		<h1>Your booking</h1>
		<p class="notice error" role="alert">{TOO_MANY_TRIES}</p>
	{:else}
		<h1>Your booking</h1>
		<p>Open your booking from the link in your confirmation email.</p>
		<Contact
			lead="For anything about your booking, contact the hotel."
			phone={site.contact_phone}
			email={site.contact_email}
		/>
	{/if}
</main>

<style>
	.manage {
		display: grid;
		gap: 1.5rem;
		max-width: 44rem;
		padding-block: 2.5rem 4rem;
	}

	h1 {
		font-size: clamp(1.75rem, 5vw, 2.5rem);
	}

	.summary,
	.cancel {
		display: grid;
		gap: 0.75rem;
		padding: 1.5rem;
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}

	dl {
		margin: 0;
		display: grid;
		gap: 0.5rem;
	}

	dl div {
		display: grid;
		grid-template-columns: 11rem 1fr;
		gap: 1rem;
	}

	dt {
		color: var(--muted);
	}

	dd {
		margin: 0;
	}

	.rooms {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 0.75rem;
	}

	.rooms li {
		padding-block: 0.75rem;
		border-top: 1px solid var(--line);
	}

	.total {
		font-weight: 600;
		font-size: 1.15rem;
	}

	.confirm {
		display: grid;
		gap: 0.75rem;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
	}

	.button.danger {
		background: var(--danger);
		border-color: var(--danger);
	}

	@media (max-width: 30rem) {
		dl div {
			grid-template-columns: 1fr;
			gap: 0;
		}
	}
</style>
