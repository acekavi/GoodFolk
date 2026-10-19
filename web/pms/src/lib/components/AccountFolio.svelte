<!--
	An account's folios in the property: its city ledger, with charges from every reservation billed to it. Once a
	folio is invoiced the next charge opens a fresh one, so the account has one open folio and any number of
	invoiced ones: one tab each, the open one first and shown, then the invoiced ones newest first. Each is read
	with its newest 500 lines and payments and refetched on its `folio:<id>` event. They take the same actions as a
	reservation's folios, gated the same way, and are invoiced here.
-->
<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import {
		accountFolioLabels,
		accountFoliosKey,
		fetchAccountFolios,
		fetchFolioContext,
		folioContextKey,
		type FolioPermissions
	} from '$lib/folio';
	import FolioView from './FolioView.svelte';

	interface Props {
		propertyId: string;
		account: { id: string; name: string };
		can: FolioPermissions;
	}

	const { propertyId, account, can }: Props = $props();

	const folios = createQuery(() => ({
		queryKey: accountFoliosKey(propertyId, account.id),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchAccountFolios(propertyId, account.id, signal)
	}));
	const context = createQuery(() => ({
		queryKey: folioContextKey(propertyId),
		queryFn: ({ signal }: { signal: AbortSignal }) => fetchFolioContext(propertyId, signal)
	}));
	const labels = $derived(accountFolioLabels(folios.data ?? [], account.name));
	let chosen = $state('');
	// The server lists the open folio first, so it is the one shown until another is chosen.
	const selected = $derived(folios.data?.find((folio) => folio.id === chosen) ?? folios.data?.[0]);
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

<section aria-label="Folio of {account.name}">
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
	{:else if folios.data.length === 0}
		<p class="hint">
			{account.name} has no folio at this property yet: it opens with the first charge billed to it.
		</p>
	{:else}
		{#if folios.data.length > 1}
			<div class="tabs" role="tablist" aria-label="Folios of {account.name}" bind:this={tabs}>
				{#each folios.data as folio (folio.id)}
					<button
						type="button"
						role="tab"
						id="account-folio-tab-{folio.id}"
						aria-selected={folio.id === selected?.id}
						aria-controls="account-folio-panel"
						tabindex={folio.id === selected?.id ? 0 : -1}
						class:secondary={folio.id !== selected?.id}
						onkeydown={keydown}
						onclick={() => (chosen = folio.id)}
						>{labels.get(folio.id)}{folio.status === 'OPEN' ? ' · open' : ''}</button
					>
				{/each}
			</div>
		{/if}
		{#if selected}
			<div
				id="account-folio-panel"
				role={folios.data.length > 1 ? 'tabpanel' : undefined}
				aria-labelledby={folios.data.length > 1 ? `account-folio-tab-${selected.id}` : undefined}
			>
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
						hasAccount={false}
					/>
				{/key}
			</div>
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
</style>
