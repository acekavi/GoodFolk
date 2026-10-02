<!--
	The confirmed stays in view that have no room: guest, type, dates and why. Assign… opens the room picker
	for one of them, limited to the rooms free for its whole stay.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { formatStay } from '$lib/reservations';
	import type { RoomType } from '$lib/rooms';
	import { needsRoomReason, type UnassignedStay } from '$lib/tape';
	import RoomAssign from './RoomAssign.svelte';

	interface Props {
		propertyId: string;
		stays: UnassignedStay[];
		roomTypes: RoomType[];
		/** `manageReservations`: without it the list is read-only. */
		manage: boolean;
	}

	const { propertyId, stays, roomTypes, manage }: Props = $props();

	let assigning = $state<string>();

	const typeCode = (id: string) => roomTypes.find((type) => type.id === id)?.code ?? '?';
</script>

<section class="needs-room" aria-label="Needs a room">
	<table>
		<thead>
			<tr><th>Guest</th><th>Type</th><th>Dates</th><th>Why</th><th></th></tr>
		</thead>
		<tbody>
			{#each stays as stay (stay.id)}
				<tr>
					<td>
						<a
							href={resolve(
								`/p/${propertyId}/reservations/${stay.reservationId}${page.url.search}`
							)}>{stay.guestName}</a
						>
					</td>
					<td>{typeCode(stay.roomTypeId)}</td>
					<td>{formatStay(stay.start, stay.end)}</td>
					<td>{needsRoomReason(stay.reason)}</td>
					<td>
						{#if manage}
							<button type="button" class="secondary" onclick={() => (assigning = stay.id)}
								>Assign…</button
							>
						{/if}
					</td>
				</tr>
				{#if assigning === stay.id}
					<tr>
						<td colspan="5">
							<RoomAssign
								{propertyId}
								{stay}
								typeCode={typeCode(stay.roomTypeId)}
								action="Assign"
								ondone={() => (assigning = undefined)}
								oncancel={() => (assigning = undefined)}
							/>
						</td>
					</tr>
				{/if}
			{/each}
		</tbody>
	</table>
</section>

<style>
	.needs-room {
		margin-bottom: var(--space);
		overflow-x: auto;
	}
</style>
