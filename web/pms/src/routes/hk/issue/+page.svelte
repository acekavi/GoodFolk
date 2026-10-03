<!--
	Report issue: room, severity and description. The room list is every active room of the property, since an
	issue is often found in a room that is not yours; "Report issue" on a room of My rooms preselects it.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys } from '$lib/api/rest';
	import { fetchBoard, hkKey, issuesKey, reportIssue, type Severity } from '$lib/housekeeping';
	import { can, fetchMe } from '$lib/session';

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
		try {
			await reportIssue(
				propertyId,
				roomId,
				severity,
				text,
				keys.keyFor({ roomId, severity, text })
			);
			const number = board.data?.find((room) => room.roomId === roomId)?.number;
			sent = `Issue reported for room ${number ?? ''}.`;
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
				{#each board.data ?? [] as room (room.roomId)}
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
