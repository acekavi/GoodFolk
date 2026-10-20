<!--
	The reservation modal's Folios tab, rendered only for `FolioView`: one tab per folio (the deposit, each
	room's stay, the account's ledger), read together and refetched on any of their `folio:<id>` events, and
	the reservation's routing for `FolioAdjust` when it is billed to an account.
-->
<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import {
		fetchFolioContext,
		fetchFolios,
		folioContextKey,
		folioLabels,
		foliosKey,
		type FolioPermissions
	} from '$lib/folio';
	import type { ReservationDetail } from '$lib/reservations';
	import FolioView from './FolioView.svelte';
	import RoutingDialog from './RoutingDialog.svelte';

	interface Props {
		propertyId: string;
		reservation: ReservationDetail;
		can: FolioPermissions;
	}

	const { propertyId, reservation, can }: Props = $props();

	const folios = createQuery(() => ({
		queryKey: foliosKey(propertyId, reservation.id, reservation.account?.id ?? null),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchFolios(propertyId, reservation.id, signal)
	}));
	const context = createQuery(() => ({
		queryKey: folioContextKey(propertyId),
		queryFn: ({ signal }: { signal: AbortSignal }) => fetchFolioContext(propertyId, signal)
	}));

	const labels = $derived(
		folioLabels(folios.data ?? [], reservation.rooms, reservation.account?.name ?? null)
	);
	let chosen = $state('');
	const selected = $derived(folios.data?.find((folio) => folio.id === chosen) ?? folios.data?.[0]);
	let routing = $state(false);
	let tabs = $state<HTMLElement>();

	/** Arrow keys move between the folio tabs, as a tab list does. */
	function keydown(event: KeyboardEvent) {
		const list = folios.data ?? [];
		const index = list.findIndex((folio) => folio.id === selected?.id);
		const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
		if (!step || index < 0) return;
		event.preventDefault();
		chosen = list[(index + step + list.length) % list.length].id;
		queueMicrotask(() => tabs?.querySelector<HTMLElement>('[aria-selected="true"]')?.focus());
	}
</script>

<section aria-label="Folios">
	{#if (folios.isError && !folios.data) || (context.isError && !context.data)}
		<p class="error" role="alert">{errorMessage(folios.error ?? context.error)}</p>
		<button
			type="button"
			onclick={() => {
				void folios.refetch();
				void context.refetch();
			}}>Retry</button
		>
	{:else if !folios.data || !context.data}
		<p>Loading…</p>
	{:else}
		{#if can.adjust && reservation.account}
			<p class="routing">
				<button type="button" class="secondary" onclick={() => (routing = true)}>Routing</button>
				<span class="hint">What {reservation.account.name} pays of the stays' charges.</span>
			</p>
		{/if}
		{#if folios.data.length === 0}
			<p class="hint">This reservation has no folios.</p>
		{:else}
			<div class="tabs" role="tablist" aria-label="Folios" bind:this={tabs}>
				{#each folios.data as folio (folio.id)}
					<button
						type="button"
						role="tab"
						id="folio-tab-{folio.id}"
						aria-selected={folio.id === selected?.id}
						aria-controls="folio-panel"
						tabindex={folio.id === selected?.id ? 0 : -1}
						class:secondary={folio.id !== selected?.id}
						onkeydown={keydown}
						onclick={() => (chosen = folio.id)}>{labels.get(folio.id)}</button
					>
				{/each}
			</div>
			{#if selected}
				<div id="folio-panel" role="tabpanel" aria-labelledby="folio-tab-{selected.id}">
					{#key selected.id}
						<FolioView
							{propertyId}
							folio={selected}
							folios={folios.data}
							{labels}
							baseCurrency={context.data.propertyTaxSettings.baseCurrency}
							currencies={context.data.propertyTaxSettings.acceptedCurrencies}
							outlets={context.data.outlets}
							{can}
							hasAccount={!!reservation.account}
						/>
					{/key}
				</div>
			{/if}
		{/if}
		{#if can.adjust && reservation.account}
			<RoutingDialog
				{propertyId}
				reservationId={reservation.id}
				accountName={reservation.account.name}
				outlets={context.data.outlets}
				bind:open={routing}
			/>
		{/if}
	{/if}
</section>

<style>
	.tabs {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem;
		margin: var(--space) 0 0;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--border);
	}
	.routing {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
		margin: var(--space) 0 0;
	}
</style>
