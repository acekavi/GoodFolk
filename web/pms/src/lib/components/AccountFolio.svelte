<!--
	An account's folio in the property: its city ledger, with charges from every reservation billed to it, read with
	its newest 500 lines and payments and refetched on its `folio:<id>` event. It takes the same actions as a
	reservation's folios, gated the same way, and is invoiced here.
-->
<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import {
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
	const labels = $derived(
		new Map((folios.data ?? []).map((folio) => [folio.id, `Account · ${account.name}`]))
	);
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
		{#each folios.data as folio (folio.id)}
			<FolioView
				{propertyId}
				{folio}
				folios={folios.data}
				{labels}
				baseCurrency={context.data.propertyTaxSettings.baseCurrency}
				currencies={context.data.propertyTaxSettings.acceptedCurrencies}
				outlets={context.data.outlets}
				{can}
				hasAccount={false}
			/>
		{/each}
	{/if}
</section>
