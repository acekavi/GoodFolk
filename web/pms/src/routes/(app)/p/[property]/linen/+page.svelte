<!--
	The property's linen, for `LinenView`: the stock of each item at each location against its par (Move records a
	receive, a transfer or, for `LinenManage`, a write-off), laundry batches sent and counted back in with their
	settle actions, the movement ledger, and for `LinenManage` the items, locations and par table. Every query here
	refetches on the server's `linen:<property>` event.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import BatchReceiveDialog from '$lib/components/BatchReceiveDialog.svelte';
	import LinenMoveDialog from '$lib/components/LinenMoveDialog.svelte';
	import {
		batchLocations,
		batchStatusText,
		cellText,
		countItems,
		fetchBatches,
		fetchLinen,
		fetchMovements,
		linenKey,
		locationKindLabel,
		MOVEMENTS_SHOWN,
		movementKindLabel,
		stockGrid,
		type BatchCountMode,
		type LinenItem,
		type LinenLocation,
		type LocationKind
	} from '$lib/linen';
	import { Pending } from '$lib/pending.svelte';
	import { fetchRooms, fetchRoomTypes, roomsKey, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
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
	// The par table's room types and a pantry's section are for the Setup tab only.
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal),
		enabled: manage
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal),
		enabled: manage
	}));

	const data = $derived(linen.data);
	const itemName = $derived(new Map((data?.linenItems ?? []).map((item) => [item.id, item.name])));
	const locationName = $derived(
		new Map((data?.linenLocations ?? []).map((location) => [location.id, location.name]))
	);
	const sectionName = $derived(
		new Map((rooms.data?.sections ?? []).map((section) => [section.id, section.name]))
	);
	const grid = $derived(
		stockGrid(data?.linenItems ?? [], data?.linenLocations ?? [], data?.linenStock ?? [])
	);
	const sendSides = $derived(batchLocations(data?.linenLocations ?? []));
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

	const pending = new Pending();
	let problem = $state('');
	let notice = $state('');
	let moving = $state(false);

	// Batches: send, then receive or settle in `BatchReceiveDialog`.
	const sendKeys = formKeys();
	const emptySend = () => ({ from: '', to: '', note: '', counts: {} as Record<string, number> });
	let send = $state(emptySend());
	let counting = $state<{ batchId: string; mode: BatchCountMode } | null>(null);
	let countOpen = $state(false);
	const countedBatch = $derived(batches.data?.find((batch) => batch.id === counting?.batchId));

	function openCount(batchId: string, mode: BatchCountMode) {
		counting = { batchId, mode };
		countOpen = true;
	}

	async function sendBatch(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		notice = '';
		const items = countItems(send.counts);
		if (items.length === 0) {
			problem = 'Enter a count for at least one item.';
			return;
		}
		const body = {
			from_location_id: send.from,
			to_location_id: send.to,
			items,
			note: send.note.trim() || null
		};
		try {
			await pending.run('send', async () => {
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/laundry-batches', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': sendKeys.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					sendKeys.failed(err);
					throw err;
				}
			});
			sendKeys.reset();
			send = emptySend();
			notice = 'Batch sent.';
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: linenKey(propertyId) });
		}
	}

	function sentAt(at: string) {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}

	// Setup: items, locations and the par table.
	const itemKeys = formKeys();
	const locationKeys = formKeys();
	let itemDraft = $state({ code: '', name: '' });
	let locationDraft = $state({ name: '', kind: 'STORE' as LocationKind, sectionId: '' });
	let editingItem = $state<{ id: string; name: string } | null>(null);
	let editingLocation = $state<{ id: string; name: string } | null>(null);
	let editingPar = $state<{ roomTypeId: string; counts: Record<string, number> } | null>(null);
	const parOf = $derived(
		new Map((data?.linenPar ?? []).map((row) => [`${row.roomTypeId}|${row.itemId}`, row.quantity]))
	);
	const KINDS: LocationKind[] = ['STORE', 'PANTRY', 'LAUNDRY', 'VENDOR'];

	async function refresh() {
		await client.invalidateQueries({ queryKey: linenKey(propertyId) });
	}

	async function addItem(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		const body = { code: itemDraft.code.trim().toUpperCase(), name: itemDraft.name.trim() };
		try {
			await pending.run('add-item', async () => {
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/linen-items', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': itemKeys.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					itemKeys.failed(err);
					throw err;
				}
			});
			itemKeys.reset();
			itemDraft = { code: '', name: '' };
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	async function addLocation(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		const body = {
			name: locationDraft.name.trim(),
			kind: locationDraft.kind.toLowerCase() as 'store' | 'pantry' | 'laundry' | 'vendor',
			section_id: locationDraft.kind === 'PANTRY' ? locationDraft.sectionId : null
		};
		try {
			await pending.run('add-location', async () => {
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/linen-locations', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': locationKeys.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					locationKeys.failed(err);
					throw err;
				}
			});
			locationKeys.reset();
			locationDraft = { name: '', kind: 'STORE', sectionId: '' };
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	/** A stale `If-Match` (412) reloads the list rather than overwrite someone else's change. */
	function updateProblem(err: unknown, what: string): string {
		return err instanceof ApiError && err.status === 412
			? `Someone else changed this ${what}. The list now shows the latest version; make your change again.`
			: errorMessage(err);
	}

	async function updateItem(item: LinenItem, body: { name?: string; active?: boolean }) {
		problem = '';
		try {
			await pending.run(item.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/linen-items/{item}', {
						params: {
							path: { property: propertyId, item: item.id },
							header: ifMatch(item.version)
						},
						body
					})
				)
			);
			if (editingItem?.id === item.id) editingItem = null;
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) editingItem = null;
			problem = updateProblem(err, 'item');
		} finally {
			await refresh();
		}
	}

	async function updateLocation(
		location: LinenLocation,
		body: { name?: string; active?: boolean }
	) {
		problem = '';
		try {
			await pending.run(location.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/linen-locations/{location}', {
						params: {
							path: { property: propertyId, location: location.id },
							header: ifMatch(location.version)
						},
						body
					})
				)
			);
			if (editingLocation?.id === location.id) editingLocation = null;
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) editingLocation = null;
			problem = updateProblem(err, 'location');
		} finally {
			await refresh();
		}
	}

	function editPar(roomTypeId: string) {
		editingPar = {
			roomTypeId,
			counts: Object.fromEntries(
				activeItems.map((item) => [item.id, parOf.get(`${roomTypeId}|${item.id}`) ?? 0])
			)
		};
	}

	/** Replaces the room type's whole par table; an item at 0 needs none. */
	async function savePar() {
		if (!editingPar) return;
		problem = '';
		const { roomTypeId, counts } = editingPar;
		try {
			await pending.run(`par-${roomTypeId}`, async () =>
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/linen-par/{room_type}', {
						params: { path: { property: propertyId, room_type: roomTypeId } },
						body: { items: countItems(counts) }
					})
				)
			);
			editingPar = null;
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await refresh();
		}
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
			onclick={() => {
				chosenTab = entry.id;
				problem = '';
				notice = '';
			}}>{entry.label}</button
		>
	{/each}
</div>

<div id="linen-panel" role="tabpanel" aria-labelledby="linen-tab-{tab}">
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}
	{#if notice}<p role="status">{notice}</p>{/if}

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
			<section aria-labelledby="send-batch">
				<h2 id="send-batch">Send a batch</h2>
				<form class="send" aria-label="Send a batch" onsubmit={sendBatch}>
					<div class="inline-form">
						<label>
							From
							<select required bind:value={send.from}>
								<option value="" disabled>Choose a store or pantry</option>
								{#each sendSides.from as location (location.id)}
									<option value={location.id}>{location.name}</option>
								{/each}
							</select>
						</label>
						<label>
							To
							<select required bind:value={send.to}>
								<option value="" disabled>Choose a laundry</option>
								{#each sendSides.to as location (location.id)}
									<option value={location.id}>{location.name}</option>
								{/each}
							</select>
						</label>
						<label>Note <input maxlength="500" bind:value={send.note} /></label>
					</div>
					<table aria-label="Counts to send">
						<thead><tr><th>Item</th><th>Count</th></tr></thead>
						<tbody>
							{#each activeItems as item (item.id)}
								<tr>
									<td>{item.name}</td>
									<td>
										<input
											type="number"
											min="0"
											max="100000"
											aria-label="{item.name} to send"
											bind:value={send.counts[item.id]}
										/>
									</td>
								</tr>
							{:else}
								<tr><td colspan="2">No linen items yet.</td></tr>
							{/each}
						</tbody>
					</table>
					<button disabled={pending.has('send')}>Send</button>
				</form>
			</section>
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
		<section aria-labelledby="setup-items">
			<h2 id="setup-items">Items</h2>
			<table aria-label="Linen items">
				<thead>
					<tr>
						<th>Code</th>
						<th>Name</th>
						<th>Active</th>
						<th><span class="visually-hidden">Actions</span></th>
					</tr>
				</thead>
				<tbody>
					{#each data.linenItems as item (item.id)}
						<tr class:inactive={!item.active}>
							<td>{item.code}</td>
							{#if editingItem?.id === item.id}
								<td>
									<input
										class="name"
										aria-label="Name for {item.code}"
										required
										maxlength="100"
										bind:value={editingItem.name}
									/>
								</td>
								<td>{item.active ? 'Active' : 'Inactive'}</td>
								<td class="actions">
									<button
										aria-label="Save {item.code}"
										disabled={pending.has(item.id) || !editingItem.name.trim()}
										onclick={() =>
											editingItem && updateItem(item, { name: editingItem.name.trim() })}
										>Save</button
									>
									<button class="secondary" onclick={() => (editingItem = null)}>Cancel</button>
								</td>
							{:else}
								<td>{item.name}</td>
								<td>{item.active ? 'Active' : 'Inactive'}</td>
								<td class="actions">
									<button
										class="secondary"
										aria-label="Edit {item.code}"
										disabled={pending.has(item.id)}
										onclick={() => (editingItem = { id: item.id, name: item.name })}>Edit</button
									>
									<button
										class="secondary"
										aria-label="{item.active ? 'Deactivate' : 'Activate'} {item.code}"
										disabled={pending.has(item.id)}
										onclick={() => updateItem(item, { active: !item.active })}
										>{item.active ? 'Deactivate' : 'Activate'}</button
									>
								</td>
							{/if}
						</tr>
					{:else}
						<tr><td colspan="4">No linen items yet.</td></tr>
					{/each}
				</tbody>
			</table>
			<form class="inline-form" aria-label="New linen item" onsubmit={addItem}>
				<label>
					Code
					<input
						required
						maxlength="20"
						pattern={'[A-Za-z0-9_]{1,20}'}
						placeholder="BATH_TOWEL"
						bind:value={itemDraft.code}
					/>
				</label>
				<label>Name <input required maxlength="100" bind:value={itemDraft.name} /></label>
				<button disabled={pending.has('add-item')}>Add item</button>
			</form>
			<p class="hint">The code can't be changed later.</p>
		</section>

		<section aria-labelledby="setup-locations">
			<h2 id="setup-locations">Locations</h2>
			<table aria-label="Linen locations">
				<thead>
					<tr>
						<th>Name</th>
						<th>Kind</th>
						<th>Section</th>
						<th>Active</th>
						<th><span class="visually-hidden">Actions</span></th>
					</tr>
				</thead>
				<tbody>
					{#each data.linenLocations as location (location.id)}
						<tr class:inactive={!location.active}>
							{#if editingLocation?.id === location.id}
								<td>
									<input
										class="name"
										aria-label="Name for {location.name}"
										required
										maxlength="100"
										bind:value={editingLocation.name}
									/>
								</td>
							{:else}
								<td>{location.name}</td>
							{/if}
							<td>{locationKindLabel(location.kind)}</td>
							<td>{location.sectionId ? (sectionName.get(location.sectionId) ?? '…') : ''}</td>
							<td>{location.active ? 'Active' : 'Inactive'}</td>
							<td class="actions">
								{#if editingLocation?.id === location.id}
									<button
										aria-label="Save {location.name}"
										disabled={pending.has(location.id) || !editingLocation.name.trim()}
										onclick={() =>
											editingLocation &&
											updateLocation(location, { name: editingLocation.name.trim() })}>Save</button
									>
									<button class="secondary" onclick={() => (editingLocation = null)}>Cancel</button>
								{:else}
									<button
										class="secondary"
										aria-label="Edit {location.name}"
										disabled={pending.has(location.id)}
										onclick={() => (editingLocation = { id: location.id, name: location.name })}
										>Edit</button
									>
									<button
										class="secondary"
										aria-label="{location.active ? 'Deactivate' : 'Activate'} {location.name}"
										disabled={pending.has(location.id)}
										onclick={() => updateLocation(location, { active: !location.active })}
										>{location.active ? 'Deactivate' : 'Activate'}</button
									>
								{/if}
							</td>
						</tr>
					{:else}
						<tr><td colspan="5">No locations yet.</td></tr>
					{/each}
				</tbody>
			</table>
			<form class="inline-form" aria-label="New linen location" onsubmit={addLocation}>
				<label>Name <input required maxlength="100" bind:value={locationDraft.name} /></label>
				<label>
					Kind
					<select bind:value={locationDraft.kind}>
						{#each KINDS as kind (kind)}
							<option value={kind}>{locationKindLabel(kind)}</option>
						{/each}
					</select>
				</label>
				{#if locationDraft.kind === 'PANTRY'}
					<label>
						Section
						<select required bind:value={locationDraft.sectionId}>
							<option value="" disabled>Choose a section</option>
							{#each rooms.data?.sections ?? [] as section (section.id)}
								<option value={section.id}>{section.name}</option>
							{/each}
						</select>
					</label>
				{/if}
				<button disabled={pending.has('add-location')}>Add location</button>
			</form>
			<p class="hint">
				A pantry serves one section's rooms; the store serves the rooms of sections with no pantry.
				A location holding stock can't be switched off.
			</p>
		</section>

		<section aria-labelledby="setup-par">
			<h2 id="setup-par">Par per room</h2>
			{#if roomTypes.error && !roomTypes.data}
				<p class="error" role="alert">{errorMessage(roomTypes.error)}</p>
				<button onclick={() => roomTypes.refetch()}>Retry</button>
			{:else if !roomTypes.data}
				<p>Loading…</p>
			{:else if activeItems.length === 0}
				<p class="hint">Add linen items to set their par.</p>
			{:else}
				<div class="scroll">
					<table aria-label="Par per room">
						<thead>
							<tr>
								<th>Room type</th>
								{#each activeItems as item (item.id)}<th>{item.name}</th>{/each}
								<th><span class="visually-hidden">Actions</span></th>
							</tr>
						</thead>
						<tbody>
							{#each roomTypes.data.filter((type) => type.active) as roomType (roomType.id)}
								<tr>
									<th scope="row">{roomType.name}</th>
									{#each activeItems as item (item.id)}
										<td>
											{#if editingPar?.roomTypeId === roomType.id}
												<input
													type="number"
													min="0"
													max="100"
													aria-label="{item.name} per {roomType.name} room"
													bind:value={editingPar.counts[item.id]}
												/>
											{:else}
												{parOf.get(`${roomType.id}|${item.id}`) ?? 0}
											{/if}
										</td>
									{/each}
									<td class="actions">
										{#if editingPar?.roomTypeId === roomType.id}
											<button
												aria-label="Save par for {roomType.name}"
												disabled={pending.has(`par-${roomType.id}`)}
												onclick={savePar}>Save</button
											>
											<button class="secondary" onclick={() => (editingPar = null)}>Cancel</button>
										{:else}
											<button
												class="secondary"
												aria-label="Edit par for {roomType.name}"
												onclick={() => editPar(roomType.id)}>Edit</button
											>
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
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
	td input.name {
		width: 12rem;
	}
</style>
