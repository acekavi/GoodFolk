<!--
	The Linen page's Setup tab, for `LinenManage`: the items (a code never changes), the locations (a pantry picks its
	section), and the par table, how many of each item one room of each room type needs.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import {
		countItems,
		linenKey,
		locationKindLabel,
		type LinenItem,
		type LinenLocation,
		type LinenPar,
		type LocationKind
	} from '$lib/linen';
	import { Pending } from '$lib/pending.svelte';
	import { fetchRooms, fetchRoomTypes, roomsKey, roomTypesKey } from '$lib/rooms';

	interface Props {
		propertyId: string;
		items: readonly LinenItem[];
		locations: readonly LinenLocation[];
		par: readonly LinenPar[];
	}

	const { propertyId, items, locations, par }: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	let problem = $state('');
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal)
	}));
	const sectionName = $derived(
		new Map((rooms.data?.sections ?? []).map((section) => [section.id, section.name]))
	);
	const activeItems = $derived(
		items.filter((item) => item.active).sort((a, b) => a.code.localeCompare(b.code))
	);

	const itemKeys = formKeys();
	const locationKeys = formKeys();
	let itemDraft = $state({ code: '', name: '' });
	let locationDraft = $state({ name: '', kind: 'STORE' as LocationKind, sectionId: '' });
	let editingItem = $state<{ id: string; name: string } | null>(null);
	let editingLocation = $state<{ id: string; name: string } | null>(null);
	let editingPar = $state<{ roomTypeId: string; counts: Record<string, number> } | null>(null);
	const parOf = $derived(
		new Map(par.map((row) => [`${row.roomTypeId}|${row.itemId}`, row.quantity]))
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
			await pending.run('add-item', () =>
				itemKeys.submit(body, async (key) =>
					unwrap(
						await rest.POST('/api/v1/properties/{property}/linen-items', {
							params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
							body
						})
					)
				)
			);
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
			await pending.run('add-location', () =>
				locationKeys.submit(body, async (key) =>
					unwrap(
						await rest.POST('/api/v1/properties/{property}/linen-locations', {
							params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
							body
						})
					)
				)
			);
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

{#if problem}<p class="error" role="alert">{problem}</p>{/if}
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
			{#each items as item (item.id)}
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
								onclick={() => editingItem && updateItem(item, { name: editingItem.name.trim() })}
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
			{#each locations as location (location.id)}
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
		A pantry serves one section's rooms; the store serves the rooms of sections with no pantry. A
		location holding stock can't be switched off.
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

<style>
	.scroll {
		overflow-x: auto;
	}
	td input.name {
		width: 12rem;
	}
</style>
