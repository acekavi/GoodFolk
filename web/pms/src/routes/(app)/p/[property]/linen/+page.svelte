<!--
	The property's linen, for `LinenView`: the stock of each item at each location against its par (Move records a
	receive, a transfer or, for `LinenManage`, a write-off), laundry batches sent and counted back in with their
	settle actions, the movement ledger, and for `LinenManage` the items, locations and par table. Every query here
	refetches on the server's `linen:<property>` event.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import BatchReceiveDialog from '$lib/components/BatchReceiveDialog.svelte';
	import BatchSendForm from '$lib/components/BatchSendForm.svelte';
	import LinenMoveDialog from '$lib/components/LinenMoveDialog.svelte';
	import LinenSetup from '$lib/components/LinenSetup.svelte';
	import {
		batchStatusText,
		cellText,
		fetchBatches,
		fetchLinen,
		fetchMovements,
		linenKey,
		locationKindLabel,
		MOVEMENTS_SHOWN,
		movementKindLabel,
		stockGrid,
		type BatchCountMode
	} from '$lib/linen';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const canMove = $derived(!!me.data && can(me.data, 'linenMove', propertyId));
	const manage = $derived(!!me.data && can(me.data, 'linenManage', propertyId));

	const linen = createQuery(() => ({
		queryKey: linenKey(propertyId),
		queryFn: ({ signal }) => fetchLinen(propertyId, signal)
	}));
	const batches = createQuery(() => ({
		queryKey: linenKey(propertyId, 'batches'),
		queryFn: ({ signal }) => fetchBatches(propertyId, signal)
	}));
	let movementsAt = $state('');
	const movements = createQuery(() => ({
		queryKey: linenKey(propertyId, 'movements', movementsAt),
		queryFn: ({ signal }) => fetchMovements(propertyId, movementsAt || null, signal)
	}));

	const data = $derived(linen.data);
	const itemName = $derived(new Map((data?.linenItems ?? []).map((item) => [item.id, item.name])));
	const locationName = $derived(
		new Map((data?.linenLocations ?? []).map((location) => [location.id, location.name]))
	);
	const grid = $derived(
		stockGrid(data?.linenItems ?? [], data?.linenLocations ?? [], data?.linenStock ?? [])
	);
	const activeItems = $derived(
		(data?.linenItems ?? [])
			.filter((item) => item.active)
			.sort((a, b) => a.code.localeCompare(b.code))
	);

	type Tab = 'stock' | 'batches' | 'movements' | 'setup';
	const tabs = $derived<{ id: Tab; label: string }[]>([
		{ id: 'stock', label: 'Stock' },
		{ id: 'batches', label: 'Batches' },
		{ id: 'movements', label: 'Movements' },
		...(manage ? [{ id: 'setup' as const, label: 'Setup' }] : [])
	]);
	let chosenTab = $state<Tab>('stock');
	const tab = $derived(tabs.some((entry) => entry.id === chosenTab) ? chosenTab : 'stock');
	let tabList = $state<HTMLElement>();

	/** Arrow keys move between the tabs, as a tab list does. */
	function tabKeydown(event: KeyboardEvent) {
		const index = tabs.findIndex((entry) => entry.id === tab);
		const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
		if (!step || index < 0) return;
		event.preventDefault();
		chosenTab = tabs[(index + step + tabs.length) % tabs.length].id;
		queueMicrotask(() => tabList?.querySelector<HTMLElement>('[aria-selected="true"]')?.focus());
	}

	let moving = $state(false);

	// Receive or settle a batch in `BatchReceiveDialog`.
	let counting = $state<{ batchId: string; mode: BatchCountMode } | null>(null);
	let countOpen = $state(false);
	const countedBatch = $derived(batches.data?.find((batch) => batch.id === counting?.batchId));

	function openCount(batchId: string, mode: BatchCountMode) {
		counting = { batchId, mode };
		countOpen = true;
	}

	function sentAt(at: string) {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}
</script>

<h1>Linen</h1>

<div class="tabs" role="tablist" aria-label="Linen" bind:this={tabList}>
	{#each tabs as entry (entry.id)}
		<button
			type="button"
			role="tab"
			id="linen-tab-{entry.id}"
			aria-selected={entry.id === tab}
			aria-controls="linen-panel"
			tabindex={entry.id === tab ? 0 : -1}
			class:secondary={entry.id !== tab}
			onkeydown={tabKeydown}
			onclick={() => (chosenTab = entry.id)}>{entry.label}</button
		>
	{/each}
</div>

<div id="linen-panel" role="tabpanel" aria-labelledby="linen-tab-{tab}">
	{#if linen.error && !data}
		<p class="error" role="alert">{errorMessage(linen.error)}</p>
		<button onclick={() => linen.refetch()}>Retry</button>
	{:else if !data}
		<p>Loading…</p>
	{:else if tab === 'stock'}
		{#if canMove}
			<p><button onclick={() => (moving = true)}>Move linen</button></p>
		{/if}
		{#if grid.rows.length === 0 || grid.columns.length === 0}
			<p class="hint">Add linen items and locations in Setup to see their stock.</p>
		{:else}
			<div class="scroll">
				<table aria-label="Stock">
					<thead>
						<tr>
							<th>Item</th>
							{#each grid.columns as location (location.id)}
								<th>
									{location.name}
									<span class="note">{locationKindLabel(location.kind)}</span>
								</th>
							{/each}
						</tr>
					</thead>
					<tbody>
						{#each grid.rows as row (row.item.id)}
							<tr>
								<th scope="row">{row.item.name} <span class="note">{row.item.code}</span></th>
								{#each row.cells as cell (cell.location.id)}
									<td class:low={cell.low}>
										{cellText(cell)}
										{#if cell.low}<span class="visually-hidden">, below par</span>{/if}
									</td>
								{/each}
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			<p class="hint">"of" shows the par: what the location's rooms need.</p>
		{/if}
		<LinenMoveDialog
			{propertyId}
			items={data.linenItems}
			locations={data.linenLocations}
			canWriteOff={manage}
			bind:open={moving}
		/>
	{:else if tab === 'batches'}
		{#if canMove}
			<BatchSendForm {propertyId} items={activeItems} locations={data.linenLocations} />
		{/if}

		<section aria-labelledby="batch-list">
			<h2 id="batch-list">Batches</h2>
			{#if batches.error && !batches.data}
				<p class="error" role="alert">{errorMessage(batches.error)}</p>
				<button onclick={() => batches.refetch()}>Retry</button>
			{:else if !batches.data}
				<p>Loading…</p>
			{:else if batches.data.length === 0}
				<p class="hint">No batches yet.</p>
			{:else}
				<div class="scroll">
					<table aria-label="Batches">
						<thead>
							<tr>
								<th>Sent</th>
								<th>From</th>
								<th>To</th>
								<th>Items</th>
								<th>Status</th>
								<th><span class="visually-hidden">Actions</span></th>
							</tr>
						</thead>
						<tbody>
							{#each batches.data as batch (batch.id)}
								<tr>
									<td>
										{sentAt(batch.sentAt)}
										{#if batch.note}<span class="note">{batch.note}</span>{/if}
									</td>
									<td>{locationName.get(batch.fromLocationId) ?? 'Unknown'}</td>
									<td>{locationName.get(batch.toLocationId) ?? 'Unknown'}</td>
									<td>
										<ul class="lines">
											{#each batch.lines as line (line.itemId)}
												<li>
													{itemName.get(line.itemId) ?? 'Unknown item'}: {line.sent} sent{#if batch.status === 'RECEIVED'},
														{line.returned} back{#if line.writtenOff > 0}, {line.writtenOff} written off{/if}{#if line.outstanding > 0},
															<strong>{line.outstanding} out</strong>{/if}{/if}
												</li>
											{/each}
										</ul>
									</td>
									<td>{batchStatusText(batch)}</td>
									<td>
										<div class="actions">
											{#if batch.status === 'SENT' && canMove}
												<button onclick={() => openCount(batch.id, 'receive')}>Receive</button>
											{:else if batch.status === 'RECEIVED' && batch.outstanding > 0}
												{#if canMove}
													<button class="secondary" onclick={() => openCount(batch.id, 'return')}
														>Return found</button
													>
												{/if}
												{#if manage}
													<button class="secondary" onclick={() => openCount(batch.id, 'write_off')}
														>Write off</button
													>
												{/if}
											{/if}
										</div>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
		{#if counting}
			<BatchReceiveDialog
				{propertyId}
				batch={countedBatch}
				mode={counting.mode}
				items={data.linenItems}
				locations={data.linenLocations}
				bind:open={countOpen}
			/>
		{/if}
	{:else if tab === 'movements'}
		<label class="filter">
			Location
			<select bind:value={movementsAt}>
				<option value="">All locations</option>
				{#each data.linenLocations as location (location.id)}
					<option value={location.id}>{location.name}</option>
				{/each}
			</select>
		</label>
		{#if movements.error && !movements.data}
			<p class="error" role="alert">{errorMessage(movements.error)}</p>
			<button onclick={() => movements.refetch()}>Retry</button>
		{:else if !movements.data}
			<p>Loading…</p>
		{:else if movements.data.length === 0}
			<p class="hint">No movements yet.</p>
		{:else}
			<p class="hint">The latest {MOVEMENTS_SHOWN} movements, newest first.</p>
			<div class="scroll">
				<table aria-label="Movements">
					<thead>
						<tr>
							<th>When</th>
							<th>Kind</th>
							<th>Item</th>
							<th>Quantity</th>
							<th>From</th>
							<th>To</th>
							<th>Reason</th>
						</tr>
					</thead>
					<tbody>
						{#each movements.data as movement (movement.id)}
							<tr>
								<td>
									{sentAt(movement.movedAt)}
									<span class="note">business date {movement.businessDate}</span>
								</td>
								<td>{movementKindLabel(movement.kind)}</td>
								<td>{itemName.get(movement.itemId) ?? 'Unknown item'}</td>
								<td>{movement.quantity}</td>
								<td>{movement.fromLocationId ? locationName.get(movement.fromLocationId) : '—'}</td>
								<td>{movement.toLocationId ? locationName.get(movement.toLocationId) : '—'}</td>
								<td>{movement.reason ?? ''}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	{:else if tab === 'setup'}
		<LinenSetup
			{propertyId}
			items={data.linenItems}
			locations={data.linenLocations}
			par={data.linenPar}
		/>
	{/if}
</div>

<style>
	.tabs {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem;
		margin-bottom: var(--space);
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--border);
	}
	.scroll {
		overflow-x: auto;
	}
	.note {
		display: block;
		color: var(--muted);
		font-size: 0.85em;
		font-weight: normal;
	}
	td.low {
		color: var(--danger);
		font-weight: 600;
	}
	.lines {
		margin: 0;
		padding-left: 1rem;
	}
	.filter {
		max-width: 16rem;
	}
</style>
