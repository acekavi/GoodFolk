<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import {
		fetchRooms,
		fetchRoomTypes,
		groupRooms,
		moveItem,
		rangeNumbers,
		roomsKey,
		roomTypesKey,
		type Room
	} from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const MAX_RANGE = 200;
	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRooms', propertyId));
	const activeTypes = $derived((roomTypes.data ?? []).filter((type) => type.active));
	const typeCode = $derived(new Map((roomTypes.data ?? []).map((type) => [type.id, type.code])));

	let groupBy = $state<'type' | 'floor'>('type');
	const groups = $derived(groupRooms(rooms.data?.rooms ?? [], roomTypes.data ?? [], groupBy));

	let error = $state('');
	// Only the row, list or form a command changes is disabled while it runs.
	const pending = new Pending();
	let bulkDialog = $state<HTMLDialogElement>();
	let bulk = $state({
		roomTypeId: '',
		prefix: '',
		first: 101,
		last: 110,
		floor: '',
		sectionId: ''
	});
	const bulkNumbers = $derived(rangeNumbers(bulk.prefix, bulk.first, bulk.last));
	let single = $state({ roomTypeId: '', number: '', floor: '' });
	let sectionName = $state('');
	const bulkForm = formKeys();
	const singleForm = formKeys();
	const sectionForm = formKeys();

	/**
	 * Runs a command under `key` (a room's id, `order`, or a form's name); shows its problem if it fails,
	 * and refetches either way.
	 */
	async function run(
		key: string,
		command: () => Promise<unknown>,
		onError?: (err: unknown) => void
	): Promise<boolean> {
		error = '';
		try {
			await pending.run(key, command);
			await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
			return true;
		} catch (err) {
			error = errorMessage(err);
			onError?.(err);
			await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
			return false;
		}
	}

	function openBulk() {
		bulk.roomTypeId ||= activeTypes[0]?.id ?? '';
		error = '';
		bulkDialog?.showModal();
	}

	async function addRange(event: SubmitEvent) {
		event.preventDefault();
		const body = {
			room_type_id: bulk.roomTypeId,
			prefix: bulk.prefix,
			first: bulk.first,
			last: bulk.last,
			floor: bulk.floor || null,
			section_id: bulk.sectionId || null
		};
		const added = await run(
			'bulk',
			async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/rooms/bulk', {
						params: {
							path: { property: propertyId },
							header: { 'Idempotency-Key': bulkForm.keyFor(body) }
						},
						body
					})
				),
			bulkForm.failed
		);
		if (added) {
			bulkForm.reset();
			bulkDialog?.close();
		}
	}

	async function addRoom(event: SubmitEvent) {
		event.preventDefault();
		const body = {
			room_type_id: single.roomTypeId || activeTypes[0]?.id,
			number: single.number,
			floor: single.floor || null
		};
		const added = await run(
			'room',
			async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/rooms', {
						params: {
							path: { property: propertyId },
							header: { 'Idempotency-Key': singleForm.keyFor(body) }
						},
						body
					})
				),
			singleForm.failed
		);
		if (added) {
			singleForm.reset();
			single.number = '';
		}
	}

	async function addSection(event: SubmitEvent) {
		event.preventDefault();
		const body = { name: sectionName };
		const added = await run(
			'section',
			async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/sections', {
						params: {
							path: { property: propertyId },
							header: { 'Idempotency-Key': sectionForm.keyFor(body) }
						},
						body
					})
				),
			sectionForm.failed
		);
		if (added) {
			sectionForm.reset();
			sectionName = '';
		}
	}

	function update(
		room: Room,
		body: {
			room_type_id?: string;
			floor?: string | null;
			section_id?: string | null;
			active?: boolean;
		}
	) {
		return run(room.id, async () =>
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/rooms/{room}', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body
				})
			)
		);
	}

	/** Moves `room` one place up or down in the property's overall room order. */
	function move(room: Room, delta: number) {
		const all = rooms.data?.rooms ?? [];
		const from = all.findIndex((r) => r.id === room.id);
		const moved = moveItem(all, from, from + delta);
		if (moved.every((r, index) => r.id === all[index].id)) return;
		return run('order', async () =>
			unwrap(
				await rest.PUT('/api/v1/properties/{property}/rooms/order', {
					params: { path: { property: propertyId } },
					body: { ids: moved.map((r) => r.id) }
				})
			)
		);
	}
</script>

<h1>Rooms</h1>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if rooms.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(rooms.error ?? roomTypes.error)}</p>
{:else if rooms.data && roomTypes.data}
	<div class="inline-form">
		<label>
			Group by
			<select bind:value={groupBy}>
				<option value="type">Room type</option>
				<option value="floor">Floor</option>
			</select>
		</label>
		{#if manage}
			<button disabled={pending.has('bulk') || activeTypes.length === 0} onclick={openBulk}
				>Add rooms…</button
			>
		{/if}
	</div>
	{#if activeTypes.length === 0}
		<p>Add a room type first.</p>
	{/if}

	{#each groups as group (group.key)}
		<h2>{group.label}</h2>
		<table>
			<thead>
				<tr>
					<th>Number</th>
					<th>Type</th>
					<th>Floor</th>
					<th>Section</th>
					<th>Status</th>
					{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
				</tr>
			</thead>
			<tbody>
				{#each group.rooms as room (room.id)}
					<tr class:inactive={!room.active}>
						<td>{room.number}</td>
						{#if manage}
							<td>
								<select
									aria-label="Type of room {room.number}"
									value={room.roomTypeId}
									disabled={pending.has(room.id)}
									onchange={(event) => update(room, { room_type_id: event.currentTarget.value })}
								>
									{#each roomTypes.data as type (type.id)}
										<option value={type.id} disabled={!type.active}>{type.code}</option>
									{/each}
								</select>
							</td>
							<td>
								<input
									aria-label="Floor of room {room.number}"
									value={room.floor ?? ''}
									disabled={pending.has(room.id)}
									onchange={(event) => update(room, { floor: event.currentTarget.value || null })}
								/>
							</td>
							<td>
								<select
									aria-label="Section of room {room.number}"
									value={room.sectionId ?? ''}
									disabled={pending.has(room.id)}
									onchange={(event) =>
										update(room, { section_id: event.currentTarget.value || null })}
								>
									<option value="">None</option>
									{#each rooms.data.sections as section (section.id)}
										<option value={section.id}>{section.name}</option>
									{/each}
								</select>
							</td>
						{:else}
							<td>{typeCode.get(room.roomTypeId)}</td>
							<td>{room.floor ?? ''}</td>
							<td>{rooms.data.sections.find((s) => s.id === room.sectionId)?.name ?? ''}</td>
						{/if}
						<td>{room.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Move room {room.number} up"
									disabled={pending.has('order')}
									onclick={() => move(room, -1)}>↑</button
								>
								<button
									class="secondary"
									aria-label="Move room {room.number} down"
									disabled={pending.has('order')}
									onclick={() => move(room, 1)}>↓</button
								>
								<button
									class="secondary"
									disabled={pending.has(room.id)}
									aria-label="{room.active ? 'Deactivate' : 'Activate'} room {room.number}"
									onclick={() => update(room, { active: !room.active })}
									>{room.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p>No rooms yet.</p>
	{/each}

	{#if manage && activeTypes.length > 0}
		<h2>Add a room</h2>
		<form class="inline-form" aria-label="New room" onsubmit={addRoom}>
			<label
				>Number <input required pattern={'[A-Za-z0-9-]{1,10}'} bind:value={single.number} /></label
			>
			<label>
				Type
				<select bind:value={single.roomTypeId}>
					{#each activeTypes as type (type.id)}<option value={type.id}>{type.code}</option>{/each}
				</select>
			</label>
			<label>Floor <input maxlength="20" bind:value={single.floor} /></label>
			<button disabled={pending.has('room')}>Add room</button>
		</form>

		<h2>Housekeeping sections</h2>
		<p>{rooms.data.sections.map((section) => section.name).join(', ') || 'None yet.'}</p>
		<form class="inline-form" aria-label="New section" onsubmit={addSection}>
			<label>Name <input required maxlength="100" bind:value={sectionName} /></label>
			<button disabled={pending.has('section')}>Add section</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<dialog bind:this={bulkDialog} aria-labelledby="bulk-title">
	<form class="form" onsubmit={addRange}>
		<h2 id="bulk-title">Add rooms</h2>
		<label>
			Room type
			<select required bind:value={bulk.roomTypeId}>
				{#each activeTypes as type (type.id)}<option value={type.id}>{type.code}</option>{/each}
			</select>
		</label>
		<label>Prefix (optional) <input maxlength="5" bind:value={bulk.prefix} /></label>
		<label>First number <input type="number" min="0" required bind:value={bulk.first} /></label>
		<label>Last number <input type="number" min="0" required bind:value={bulk.last} /></label>
		<label>Floor <input maxlength="20" bind:value={bulk.floor} /></label>
		<label>
			Section
			<select bind:value={bulk.sectionId}>
				<option value="">None</option>
				{#each rooms.data?.sections ?? [] as section (section.id)}
					<option value={section.id}>{section.name}</option>
				{/each}
			</select>
		</label>
		{#if bulkNumbers.length > MAX_RANGE}
			<p class="error">At most {MAX_RANGE} rooms at a time.</p>
		{:else if bulkNumbers.length > 0}
			<p>{bulkNumbers[0]} – {bulkNumbers.at(-1)} ({bulkNumbers.length} rooms)</p>
		{/if}
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<div class="actions">
			<button
				disabled={pending.has('bulk') || bulkNumbers.length === 0 || bulkNumbers.length > MAX_RANGE}
				>Add {bulkNumbers.length} rooms</button
			>
			<button type="button" class="secondary" onclick={() => bulkDialog?.close()}>Cancel</button>
		</div>
	</form>
</dialog>
