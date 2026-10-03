<!--
	Report issue: room, severity and description. The room list is every active room of the property, since an
	issue is often found in a room that is not yours; "Report issue" on a room of My rooms preselects it.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys } from '$lib/api/rest';
	import { sync } from '$lib/hk-sync.svelte';
	import { snapshots, type RoomChoice } from '$lib/hk-snapshot';
	import { fetchBoard, hkKey, issuesKey, reportIssue, type Severity } from '$lib/housekeeping';
	import { can, fetchMe, ownerOf } from '$lib/session';

	const SEVERITIES: Severity[] = ['LOW', 'MEDIUM', 'HIGH'];
	const SEVERITY_LABELS: Record<Severity, string> = { LOW: 'Low', MEDIUM: 'Medium', HIGH: 'High' };

	const propertyId = $derived(page.url.searchParams.get('property') ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const mayReport = $derived(!!me.data && can(me.data, 'roomIssuesReport', propertyId));
	const board = createQuery(() => ({
		queryKey: hkKey(propertyId),
		queryFn: ({ signal }) => fetchBoard(propertyId, signal),
		enabled: !!propertyId
	}));

	// The rooms to choose from without a connection: those saved when this page last had them, else My rooms.
	let saved = $state<RoomChoice[]>([]);
	$effect(() => {
		const viewer = sync.viewer;
		if (!viewer) {
			saved = [];
			return;
		}
		void (async () => {
			const choices = await snapshots.loadChoices(propertyId, viewer);
			const snapshot = choices ? undefined : await snapshots.load(propertyId || null, viewer);
			saved = choices ?? snapshot?.rooms.map(({ roomId, number }) => ({ roomId, number })) ?? [];
		})();
	});
	$effect(() => {
		if (board.data && propertyId && me.data) {
			void snapshots.saveChoices(
				propertyId,
				ownerOf(me.data),
				board.data.map(({ roomId, number }) => ({ roomId, number }))
			);
		}
	});
	const choices = $derived<RoomChoice[]>(board.data ?? saved);

	let roomId = $state(page.url.searchParams.get('room') ?? '');
	let severity = $state<Severity>('LOW');
	let description = $state('');
	let busy = $state(false);
	let error = $state('');
	let sent = $state('');
	const keys = formKeys();

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		sent = '';
		const text = description.trim();
		const number = choices.find((room) => room.roomId === roomId)?.number ?? '';
		const key = keys.keyFor({ roomId, severity, text });
		// Without a connection the report waits on the device, under the key of this very submit.
		const queue = () =>
			sync.enqueue({
				kind: 'issue',
				propertyId,
				roomId,
				roomNumber: number,
				severity,
				description: text,
				key
			});
		try {
			if (!sync.online || sync.waiting.length > 0) {
				await queue();
				sent = `Issue for room ${number} saved on this device; it is sent when you are back online.`;
			} else {
				try {
					await reportIssue(propertyId, roomId, severity, text, key);
					sent = `Issue reported for room ${number}.`;
				} catch (err) {
					if (err instanceof ApiError) throw err;
					await queue();
					sent = `Issue for room ${number} saved on this device; it is sent when you are back online.`;
				}
			}
			description = '';
			roomId = '';
			severity = 'LOW';
			keys.reset();
			void client.invalidateQueries({ queryKey: issuesKey(propertyId) });
			void client.invalidateQueries({ queryKey: hkKey(propertyId) });
		} catch (err) {
			keys.failed(err);
			error =
				err instanceof ApiError && err.status === 403
					? 'Your role cannot report issues at this property.'
					: errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<h1>Report issue</h1>
{#if me.data && !mayReport}
	<p class="error" role="alert">Your role cannot report issues at this property.</p>
{:else}
	{#if sent}<p role="status">{sent}</p>{/if}
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	{#if board.error}<p class="error" role="alert">{errorMessage(board.error)}</p>{/if}
	<form class="form" aria-label="Report an issue" onsubmit={submit}>
		<label>
			Room
			<select bind:value={roomId} required>
				<option value="" disabled>Choose a room</option>
				{#each choices as room (room.roomId)}
					<option value={room.roomId}>{room.number}</option>
				{/each}
			</select>
		</label>
		<label>
			Severity
			<select bind:value={severity}>
				{#each SEVERITIES as value (value)}
					<option {value}>{SEVERITY_LABELS[value]}</option>
				{/each}
			</select>
		</label>
		<label>
			Description
			<textarea bind:value={description} required maxlength="2000" rows="5"></textarea>
		</label>
		<button type="submit" disabled={busy || !roomId || !description.trim()}>Report issue</button>
	</form>
{/if}

<style>
	.form {
		max-width: none;
	}
	select,
	textarea,
	button {
		min-height: 48px;
		font-size: 1rem;
	}
	textarea {
		font: inherit;
		color: inherit;
		padding: 0.45rem 0.6rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg);
	}
</style>
