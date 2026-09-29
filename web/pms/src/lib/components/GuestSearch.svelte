<!--
	A guest search box: results appear a moment after typing stops, matching the new-reservation
	screen's guest step (`reservations/new/+page.svelte`). Picking a result calls `onChoose`; this
	component never creates a guest itself.
-->
<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { fetchGuests, guestsKey, idDocText, residencyLabel, type Guest } from '$lib/reservations';

	const SEARCH_DELAY_MS = 300;

	interface Props {
		propertyId: string;
		/** Guest ids left out of the results, e.g. the room's primary guest and its current occupants. */
		exclude?: readonly string[];
		onChoose: (guest: Guest) => void;
	}

	let { propertyId, exclude = [], onChoose }: Props = $props();

	let input = $state<HTMLInputElement>();
	let text = $state('');
	let searched = $state('');
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => () => clearTimeout(timer));
	$effect(() => input?.focus());

	const guests = createQuery(() => ({
		queryKey: guestsKey(propertyId, searched),
		queryFn: ({ signal }: { signal: AbortSignal }) => fetchGuests(propertyId, searched, 10, signal),
		enabled: searched !== ''
	}));

	const results = $derived((guests.data ?? []).filter((guest) => !exclude.includes(guest.id)));

	function find() {
		clearTimeout(timer);
		timer = setTimeout(() => (searched = text.trim()), SEARCH_DELAY_MS);
	}

	function findNow(event: SubmitEvent) {
		event.preventDefault();
		clearTimeout(timer);
		searched = text.trim();
	}

	function guestName(guest: Guest): string {
		return [guest.firstName, guest.lastName].filter(Boolean).join(' ');
	}
</script>

<form class="inline-form" aria-label="Guest search" onsubmit={findNow}>
	<label>
		Find a guest
		<input
			type="search"
			placeholder="Name, email or phone"
			bind:this={input}
			bind:value={text}
			oninput={find}
		/>
	</label>
</form>
{#if searched}
	{#if guests.isError}
		<p class="error" role="alert">{errorMessage(guests.error)}</p>
	{:else if guests.data}
		<ul class="guests" aria-label="Guests found">
			{#each results as guest (guest.id)}
				<li>
					<button type="button" class="secondary" onclick={() => onChoose(guest)}>
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
				<li class="hint">No guest matches “{searched}”.</li>
			{/each}
		</ul>
	{:else}
		<p>Searching…</p>
	{/if}
{/if}

<style>
	.guests {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.25rem;
		max-width: 30rem;
	}
	.guests button {
		display: flex;
		gap: 0.5rem;
		width: 100%;
		text-align: left;
	}
</style>
