<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { fetchRoomTypes, moveItem, roomTypesKey, type RoomType } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRooms', propertyId));

	type Capacity = Pick<RoomType, 'baseOccupancy' | 'maxAdults' | 'maxChildren' | 'maxOccupancy'>;
	const emptyDraft = () => ({
		code: '',
		name: '',
		baseOccupancy: 2,
		maxAdults: 2,
		maxChildren: 0,
		maxOccupancy: 2
	});
	let draft = $state(emptyDraft());
	let editing = $state<({ id: string; version: number; name: string } & Capacity) | null>(null);
	let dragged = $state<number | null>(null);
	let error = $state('');
	let busy = $state(false);
	const createForm = formKeys();

	function capacity(values: Capacity) {
		return {
			base_occupancy: values.baseOccupancy,
			max_adults: values.maxAdults,
			max_children: values.maxChildren,
			max_occupancy: values.maxOccupancy
		};
	}

	/** Runs a command, shows its problem if it fails, and refetches after a version conflict. */
	async function run(command: () => Promise<unknown>): Promise<boolean> {
		busy = true;
		error = '';
		try {
			await command();
			await client.invalidateQueries({ queryKey: roomTypesKey(propertyId) });
			return true;
		} catch (err) {
			error = errorMessage(err);
			if (err instanceof ApiError && err.status === 412) {
				editing = null;
			}
			await client.invalidateQueries({ queryKey: roomTypesKey(propertyId) });
			return false;
		} finally {
			busy = false;
		}
	}

	async function create(event: SubmitEvent) {
		event.preventDefault();
		const body = { code: draft.code.toUpperCase(), name: draft.name, ...capacity(draft) };
		const success = await run(async () => {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/room-types', {
					params: {
						path: { property: propertyId },
						header: { 'Idempotency-Key': createForm.keyFor(body) }
					},
					body
				})
			);
		});
		if (success) {
			draft = emptyDraft();
			createForm.reset();
		}
	}

	function update(type: RoomType, body: { name?: string; active?: boolean } & object) {
		return run(async () => {
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/room-types/{room_type}', {
					params: {
						path: { property: propertyId, room_type: type.id },
						header: ifMatch(type.version)
					},
					body
				})
			);
			editing = null;
		});
	}

	async function reorder(from: number, to: number) {
		const current = roomTypes.data ?? [];
		const moved = moveItem(current, from, to);
		if (moved.every((type, index) => type.id === current[index].id)) return;
		// Show the new order at once; the refetch after the command confirms it.
		client.setQueryData(roomTypesKey(propertyId), moved);
		const success = await run(async () => {
			unwrap(
				await rest.PUT('/api/v1/properties/{property}/room-types/order', {
					params: { path: { property: propertyId } },
					body: { ids: moved.map((type) => type.id) }
				})
			);
		});
		if (!success) {
			client.setQueryData(roomTypesKey(propertyId), current);
		}
	}
</script>

<h1>Room types</h1>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if roomTypes.error}
	<p class="error" role="alert">{errorMessage(roomTypes.error)}</p>
{:else if roomTypes.data}
	<table>
		<thead>
			<tr>
				<th>Code</th>
				<th>Name</th>
				<th>Base</th>
				<th>Adults</th>
				<th>Children</th>
				<th>Max</th>
				<th>Status</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each roomTypes.data as type, index (type.id)}
				<tr
					draggable={manage && !editing}
					class:inactive={!type.active}
					ondragstart={() => (dragged = index)}
					ondragover={(event) => event.preventDefault()}
					ondrop={() => {
						if (dragged !== null) void reorder(dragged, index);
						dragged = null;
					}}
				>
					<td>{type.code}</td>
					{#if editing?.id === type.id}
						<td><input aria-label="Name of {type.code}" bind:value={editing.name} /></td>
						<td
							><input
								aria-label="Base occupancy of {type.code}"
								type="number"
								min="1"
								bind:value={editing.baseOccupancy}
							/></td
						>
						<td
							><input
								aria-label="Maximum adults of {type.code}"
								type="number"
								min="1"
								bind:value={editing.maxAdults}
							/></td
						>
						<td
							><input
								aria-label="Maximum children of {type.code}"
								type="number"
								min="0"
								bind:value={editing.maxChildren}
							/></td
						>
						<td
							><input
								aria-label="Maximum occupancy of {type.code}"
								type="number"
								min="1"
								bind:value={editing.maxOccupancy}
							/></td
						>
					{:else}
						<td>{type.name}</td>
						<td>{type.baseOccupancy}</td>
						<td>{type.maxAdults}</td>
						<td>{type.maxChildren}</td>
						<td>{type.maxOccupancy}</td>
					{/if}
					<td>{type.active ? 'Active' : 'Inactive'}</td>
					{#if manage}
						<td class="actions">
							{#if editing?.id === type.id}
								<button
									disabled={busy}
									aria-label="Save {type.code}"
									onclick={() =>
										editing && update(type, { name: editing.name, ...capacity(editing) })}
									>Save</button
								>
								<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
							{:else}
								<button
									class="secondary"
									aria-label="Move {type.code} up"
									disabled={busy || index === 0}
									onclick={() => reorder(index, index - 1)}>↑</button
								>
								<button
									class="secondary"
									aria-label="Move {type.code} down"
									disabled={busy || index === roomTypes.data.length - 1}
									onclick={() => reorder(index, index + 1)}>↓</button
								>
								<button
									class="secondary"
									aria-label="Edit {type.code}"
									disabled={busy}
									onclick={() => (editing = { ...type })}>Edit</button
								>
								<button
									class="secondary"
									disabled={busy}
									aria-label="{type.active ? 'Deactivate' : 'Activate'} {type.code}"
									onclick={() => update(type, { active: !type.active })}
									>{type.active ? 'Deactivate' : 'Activate'}</button
								>
							{/if}
						</td>
					{/if}
				</tr>
			{:else}
				<tr><td colspan="8">No room types yet.</td></tr>
			{/each}
		</tbody>
	</table>
	{#if manage}
		<p class="hint">Drag a row, or use the arrows, to change the order rooms are listed in.</p>
		<form class="inline-form" aria-label="New room type" onsubmit={create}>
			<label>Code <input required pattern={'[A-Za-z0-9]{1,10}'} bind:value={draft.code} /></label>
			<label>Name <input required maxlength="100" bind:value={draft.name} /></label>
			<label>Base <input type="number" min="1" max="50" bind:value={draft.baseOccupancy} /></label>
			<label>Adults <input type="number" min="1" max="50" bind:value={draft.maxAdults} /></label>
			<label>Children <input type="number" min="0" max="50" bind:value={draft.maxChildren} /></label
			>
			<label>Max <input type="number" min="1" max="50" bind:value={draft.maxOccupancy} /></label>
			<button disabled={busy}>Add room type</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}
