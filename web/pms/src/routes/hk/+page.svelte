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
	import { sync } from '$lib/hk-sync.svelte';
	import { snapshots, type Snapshot } from '$lib/hk-snapshot';
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
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe, ownerOf } from '$lib/session';

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
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal),
		enabled: !!me.data?.current_tenant
	}));

	// The last list read online, kept on the device: it is what shows when there is no connection.
	let saved = $state<Snapshot>();
	$effect(() => {
		const viewer = sync.viewer;
		saved = undefined;
		if (viewer) void snapshots.load(propertyId || null, viewer).then((found) => (saved = found));
	});
	$effect(() => {
		const property = properties.data?.find((candidate) => candidate.id === propertyId);
		if (!rooms.data || !roomTypes.data || !me.data || !property) return;
		void snapshots.save({
			owner: ownerOf(me.data),
			propertyId,
			propertyName: property.name,
			businessDate: property.businessDate,
			savedAt: rooms.dataUpdatedAt,
			supervise,
			rooms: rooms.data,
			typeCodes: roomTypes.data.map((type) => [type.id, type.code])
		});
	});
	// Saved data stands in when there is no connection or the API cannot be reached, never after a 401.
	const signedOut = $derived(rooms.error instanceof ApiError && rooms.error.status === 401);
	const stale = $derived(
		!rooms.data && !signedOut && (!sync.online || !!rooms.error) ? saved : undefined
	);
	const list = $derived(rooms.data ?? stale?.rooms);
	const typeCode = $derived(
		new Map(roomTypes.data ? roomTypes.data.map((type) => [type.id, type.code]) : stale?.typeCodes)
	);
	const asSupervisor = $derived(me.data ? supervise : (stale?.supervise ?? false));
	// Shown whenever the list is not a fresh read, with its date, and a warning when it is for an earlier day.
	const takenAt = $derived(
		stale?.savedAt ?? (!sync.online || rooms.error ? rooms.dataUpdatedAt : 0)
	);
	const earlierDay = $derived(
		!!stale && stale.businessDate < new Date().toLocaleDateString('en-CA')
	);
	const taken = (time: number) =>
		`${new Date(time).toLocaleDateString([], { weekday: 'short', day: 'numeric', month: 'short' })} at ${new Date(time).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;

	/** The rooms as they will be once what waits has been sent: a queued change shows at once. */
	const queued = $derived(
		new Map(
			sync.waiting
				.filter((item) => item.kind === 'condition' && item.propertyId === propertyId)
				.map((item) => [item.roomId, item])
		)
	);

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
		const behind = queued.has(room.roomId);
		const queue = () =>
			sync.enqueue({
				kind: 'condition',
				propertyId,
				roomId: room.roomId,
				roomNumber: room.number,
				condition,
				// Behind a waiting change on this room, it is based on what that change produces.
				version: behind ? null : room.conditionVersion
			});
		try {
			// Changes are sent in the order they were made, so nothing jumps ahead of what waits.
			if (!sync.online || sync.waiting.length > 0) await queue();
			else await setCondition(propertyId, room.roomId, condition, room.conditionVersion);
		} catch (err) {
			if (err instanceof ApiError) message = `Room ${room.number}: ${explain(err)}`;
			else await queue();
		} finally {
			busyRoom = '';
			if (sync.online) await client.invalidateQueries({ queryKey: hkKey(propertyId) });
		}
	}
</script>

<h1>My rooms</h1>
{#if message}<p class="error" role="alert">{message}</p>{/if}
{#if takenAt}
	<p class="hint" role="status">
		Showing the list as it was on {taken(takenAt)}.{earlierDay ? " This is yesterday's list." : ''}
	</p>
{/if}
{#if !list && rooms.isPending}
	<p>Loading…</p>
{:else if !list && rooms.error}
	<p class="error" role="alert">{errorMessage(rooms.error)}</p>
	<button onclick={() => rooms.refetch()}>Retry</button>
{:else if !list}
	<p>Loading…</p>
{:else if list.length === 0}
	<p class="hint">No rooms are assigned to you today.</p>
{:else}
	<ul aria-label="Rooms">
		{#each list as listed (listed.roomId)}
			{@const waiting = queued.get(listed.roomId)}
			{@const room =
				waiting?.kind === 'condition' ? { ...listed, condition: waiting.condition } : listed}
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
					{#if waiting}<span>Waiting to be sent</span>{/if}
				</p>
				<div class="steps">
					{#each nextConditions(asSupervisor ? 'supervisor' : 'housekeeper', room.condition) as condition (condition)}
						<button
							type="button"
							disabled={busyRoom === room.roomId}
							onclick={() => move(listed, condition)}
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
