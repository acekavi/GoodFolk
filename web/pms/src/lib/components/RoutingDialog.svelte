<!--
	Which of the reservation's charges its account pays, by category; every other charge stays on the stays'
	guest folios. Opens with the routing in force, and replaces it on save. Changes only charges posted after.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { rest, unwrap } from '$lib/api/rest';
	import { categories, fetchRouting, routingKey, type Outlet } from '$lib/folio';

	interface Props {
		propertyId: string;
		reservationId: string;
		accountName: string;
		outlets: readonly Outlet[];
		open: boolean;
	}

	let { propertyId, reservationId, accountName, outlets, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	const routing = createQuery(() => ({
		queryKey: routingKey(reservationId),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchRouting(propertyId, reservationId, signal),
		enabled: open
	}));
	let dialog = $state<HTMLDialogElement>();
	let chosen = $state<string[]>([]);
	let loaded = $state(false);
	let problem = $state('');
	let busy = $state(false);

	// The routing in force, plus any category it names that the form would not offer (an outlet since closed).
	const offered = $derived.by(() => {
		const list = categories(outlets);
		for (const category of routing.data ?? []) {
			if (!list.some((entry) => entry.value === category)) {
				list.push({ value: category, label: category });
			}
		}
		return list;
	});

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			loaded = false;
			problem = '';
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	$effect(() => {
		if (open && !loaded && routing.data && !routing.isFetching) {
			chosen = [...routing.data];
			loaded = true;
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		problem = '';
		try {
			unwrap(
				await rest.PUT('/api/v1/properties/{property}/reservations/{reservation}/routing', {
					params: { path: { property: propertyId, reservation: reservationId } },
					body: { categories: chosen }
				})
			);
			void client.invalidateQueries({ queryKey: routingKey(reservationId) });
			open = false;
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="routing-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="routing-title">Routing</h2>
		<p class="hint">
			Charges of the categories ticked go to {accountName}'s folio; the rest stay on each room's
			folio. Only charges posted from now on follow a change.
		</p>
		{#if routing.isError && !routing.data}
			<p class="error" role="alert">{errorMessage(routing.error)}</p>
			<button type="button" onclick={() => routing.refetch()}>Retry</button>
		{:else if !loaded}
			<p>Loading…</p>
		{:else}
			<fieldset>
				<legend>{accountName} pays</legend>
				{#each offered as entry (entry.value)}
					<label class="check">
						<input type="checkbox" value={entry.value} bind:group={chosen} />
						{entry.label}
					</label>
				{/each}
			</fieldset>
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<div class="actions">
				<button disabled={busy}>Save routing</button>
				<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
			</div>
		{/if}
	</form>
</dialog>

<style>
	fieldset {
		display: grid;
		gap: 0.35rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.check {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		color: var(--text);
	}
</style>
