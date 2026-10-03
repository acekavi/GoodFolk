<!--
	The housekeeper app's connection banner: whether the device is offline, how many changes wait to be sent,
	a prompt to sign in again when the session expired, and each conflict with Retry and Discard. A conflict
	says what happened to the room, from the current list.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import type { QueueItem } from '$lib/hk-queue';
	import { sync } from '$lib/hk-sync.svelte';
	import { conditionLabel, fetchMyRooms, myRoomsKey, stepLabel } from '$lib/housekeeping';

	let { propertyId }: { propertyId: string } = $props();

	const client = useQueryClient();
	const rooms = createQuery(() => ({
		queryKey: myRoomsKey(propertyId),
		queryFn: ({ signal }) => fetchMyRooms(propertyId, signal),
		enabled: !!propertyId
	}));

	const here = $derived(sync.items.filter((item) => item.propertyId === propertyId));
	const waiting = $derived(here.filter((item) => item.status === 'waiting'));
	const conflicts = $derived(here.filter((item) => item.status === 'conflict'));

	function describe(item: QueueItem): string {
		const room = `Room ${item.roomNumber}`;
		if (item.kind === 'issue') {
			return `${room}: your issue report was not sent (${why(item.conflictStatus)}).`;
		}
		const step = stepLabel(item.condition);
		if (item.version === null)
			return `${room}: an earlier change was not applied, so “${step}” was not either.`;
		const current = rooms.data?.find((candidate) => candidate.roomId === item.roomId);
		if (item.conflictStatus === 412 && current) {
			return `${room} was changed to ${conditionLabel(current.condition)} by someone else; your ${step} was not applied.`;
		}
		return `${room}: your ${step} was not applied (${why(item.conflictStatus)}).`;
	}

	function why(status: number | undefined): string {
		if (status === 403) return 'the room is not assigned to you';
		if (status === 412) return 'the room was changed by someone else';
		if (status === 409) return 'the server refused it';
		return 'it can no longer be applied';
	}

	let busy = $state(false);

	async function retry(item: QueueItem) {
		busy = true;
		try {
			// The condition version now, not the one the change was made at.
			await client.refetchQueries({ queryKey: myRoomsKey(propertyId) });
			const version =
				rooms.data?.find((candidate) => candidate.roomId === item.roomId)?.conditionVersion ?? 0;
			await sync.retry(item.id, version);
		} finally {
			busy = false;
		}
	}
</script>

{#if !sync.online || waiting.length > 0 || conflicts.length > 0 || sync.signIn}
	<section class="banner" aria-label="Connection">
		{#if !sync.online}
			<p role="status">
				You are offline. Changes are kept on this device and sent when you are back.
			</p>
		{/if}
		{#if waiting.length > 0}
			<p role="status">
				{waiting.length}
				{waiting.length === 1 ? 'change' : 'changes'} waiting to be sent.
			</p>
		{/if}
		{#if sync.signIn}
			<p role="alert">
				Your session has expired.
				<a href={resolve('/login')}>Sign in</a> to send the waiting changes.
			</p>
		{/if}
		{#if conflicts.length > 0}
			<ul aria-label="Conflicts">
				{#each conflicts as item (item.id)}
					<li>
						<p role="alert">{describe(item)}</p>
						<div class="actions">
							<button type="button" disabled={busy || !sync.online} onclick={() => retry(item)}>
								Retry
							</button>
							<button type="button" disabled={busy} onclick={() => sync.discard(item.id)}>
								Discard
							</button>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</section>
{/if}

<style>
	.banner {
		display: grid;
		gap: 0.5rem;
		margin-bottom: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	p {
		margin: 0;
	}
	ul {
		display: grid;
		gap: 0.5rem;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	li {
		display: grid;
		gap: 0.5rem;
	}
	li p {
		color: var(--danger);
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	.actions button {
		flex: 1;
		min-height: 48px;
	}
</style>
