<!--
	My rooms: the signed-in user's rooms for the business date in the server's order, each with the next step
	as one large button. A housekeeper has the two steps, "Start cleaning" and "Done"; a supervisor any other
	condition. The list reads under the property's `hk:` key, so a room changing anywhere refreshes it.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import {
		conditionLabel,
		fetchMyRooms,
		hkKey,
		myRoomsKey,
		nextConditions,
		occupancyLabel,
		setCondition,
		stepLabel,
		type Condition,
		type MyRoom
	} from '$lib/housekeeping';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.url.searchParams.get('property') ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const supervise = $derived(!!me.data && can(me.data, 'housekeepingSupervise', propertyId));
	const rooms = createQuery(() => ({
		queryKey: myRoomsKey(propertyId),
		queryFn: ({ signal }) => fetchMyRooms(propertyId, signal),
		enabled: !!propertyId
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal),
		enabled: !!propertyId
	}));
	const typeCode = $derived(new Map((roomTypes.data ?? []).map((type) => [type.id, type.code])));

	let busyRoom = $state('');
	let message = $state('');

	function explain(err: unknown): string {
		if (err instanceof ApiError) {
			if (err.status === 403) return 'This room is not assigned to you today.';
			if (err.status === 412) {
				return 'This room was changed by someone else. The list is updated; check it and try again.';
			}
		}
		return errorMessage(err);
	}

	async function move(room: MyRoom, condition: Condition) {
		busyRoom = room.roomId;
		message = '';
		try {
			await setCondition(propertyId, room.roomId, condition, room.conditionVersion);
		} catch (err) {
			message = `Room ${room.number}: ${explain(err)}`;
		} finally {
			busyRoom = '';
			await client.invalidateQueries({ queryKey: hkKey(propertyId) });
		}
	}
</script>

<h1>My rooms</h1>
{#if message}<p class="error" role="alert">{message}</p>{/if}
{#if rooms.isPending}
	<p>Loading…</p>
{:else if rooms.error}
	<p class="error" role="alert">{errorMessage(rooms.error)}</p>
	<button onclick={() => rooms.refetch()}>Retry</button>
{:else if rooms.data.length === 0}
	<p class="hint">No rooms are assigned to you today.</p>
{:else}
	<ul aria-label="Rooms">
		{#each rooms.data as room (room.roomId)}
			<li data-condition={room.condition}>
				<div class="head">
					<strong class="number">{room.number}</strong>
					<span class="condition">{conditionLabel(room.condition)}</span>
				</div>
				<p class="meta">
					{typeCode.get(room.roomTypeId) ?? ''}
					<span>{occupancyLabel(room.occupancy)}</span>
					{#if room.blockKind}<span>Blocked</span>{/if}
					{#if room.openIssues > 0}<span>{room.openIssues} open issues</span>{/if}
				</p>
				<div class="steps">
					{#each nextConditions(supervise ? 'supervisor' : 'housekeeper', room.condition) as condition (condition)}
						<button
							type="button"
							disabled={busyRoom === room.roomId}
							onclick={() => move(room, condition)}
						>
							{stepLabel(condition)}
						</button>
					{/each}
					<a class="report" href={resolve(`/hk/issue?property=${propertyId}&room=${room.roomId}`)}
						>Report issue</a
					>
				</div>
			</li>
		{/each}
	</ul>
{/if}

<style>
	ul {
		display: grid;
		gap: var(--space);
		padding: 0;
		list-style: none;
	}
	li {
		display: grid;
		gap: 0.5rem;
		padding: var(--space);
		border: 1px solid var(--border);
		border-left-width: 6px;
		border-radius: var(--radius);
		background: var(--surface);
	}
	li[data-condition='DIRTY'] {
		border-left-color: var(--danger);
	}
	li[data-condition='CLEANING'] {
		border-left-color: var(--stay-tentative);
	}
	li[data-condition='CLEAN'],
	li[data-condition='INSPECTED'] {
		border-left-color: var(--stay-checked-in);
	}
	.head {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
	}
	.number {
		font-size: 1.5rem;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.75rem;
		margin: 0;
		color: var(--muted);
	}
	.steps {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}
	.steps button,
	.report {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 48px;
		padding: 0 1rem;
	}
	.steps button {
		flex: 1;
		font-weight: 600;
	}
	.report {
		color: var(--accent);
	}
</style>
