<!--
	Puts a stay in one of the rooms free for all of its nights, for Needs a room (Assign) and a bar's Move to
	room. It is the room picker of the reservation modal (`freeRooms`, then `assign` with If-Match), limited to
	the stay's own room type. A 409 or 422 shows the server's reason here; a 412 means the stay changed since
	it was shown, so the lists refetch and the picker asks to try again.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { Pending } from '$lib/pending.svelte';
	import { fetchFreeRooms, freeRoomsKey, reservationListsKey } from '$lib/reservations';
	import { assignStay } from '$lib/tape';

	interface Props {
		propertyId: string;
		/** The booked room to assign or move; `version` is sent as If-Match. */
		stay: { id: string; roomTypeId: string; start: string; end: string; version: number };
		/** The stay's room type code, for the empty message. */
		typeCode: string;
		/** The submit button's label: Assign or Move. */
		action: string;
		/** The stay is in its new room. */
		ondone: () => void;
		oncancel: () => void;
	}

	const { propertyId, stay, typeCode, action, ondone, oncancel }: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	let choice = $state('');
	let problem = $state('');

	const free = createQuery(() => ({
		queryKey: freeRoomsKey(propertyId, stay.roomTypeId, stay.start, stay.end),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchFreeRooms(propertyId, stay.roomTypeId, stay.start, stay.end, signal)
	}));
	// Never leave the choice on a room the list no longer offers, so submitting never books another room.
	$effect(() => {
		const rooms = free.data;
		if (rooms && choice && !rooms.some((room) => room.id === choice)) choice = '';
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		try {
			await pending.run('assign', () => assignStay(propertyId, stay.id, stay.version, choice));
		} catch (err) {
			problem =
				err instanceof ApiError && err.status === 412
					? 'Someone else changed this stay. It now shows the latest version; check it and try again.'
					: errorMessage(err);
			return;
		} finally {
			// The server's events refetch these too; invalidating now shows the change at once.
			await Promise.all([
				client.invalidateQueries({ queryKey: ['tape', propertyId] }),
				client.invalidateQueries({ queryKey: ['tape-unassigned', propertyId] }),
				client.invalidateQueries({ queryKey: reservationListsKey(propertyId) }),
				client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
			]);
		}
		ondone();
	}
</script>

<form class="inline-form" aria-label="{action} a room" onsubmit={submit}>
	{#if free.isError}
		<p class="error" role="alert">{errorMessage(free.error)}</p>
	{:else if free.data && free.data.length === 0}
		<p>No {typeCode} room is free for these nights.</p>
	{:else}
		<label>
			Room
			<select required bind:value={choice} disabled={!free.data}>
				<option value="" disabled>Choose a room</option>
				{#each free.data ?? [] as option (option.id)}
					<option value={option.id}
						>{option.number}{option.section ? ` · ${option.section}` : ''}</option
					>
				{/each}
			</select>
		</label>
		<button disabled={!choice || pending.has('assign')}>{action}</button>
	{/if}
	<button type="button" class="secondary" onclick={oncancel}>Cancel</button>
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}
</form>
