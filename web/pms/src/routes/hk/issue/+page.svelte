<!--
	Report issue: room, severity, description and up to five photos. The room list is every active room of the
	property, since an issue is often found in a room that is not yours; "Report issue" on a room of My rooms
	preselects it. Photos are uploaded once the report has made the issue, below the form, where more can be
	added. They need a connection: a report saved on the device carries its text only.
-->
<script lang="ts">
	import { beforeNavigate } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys } from '$lib/api/rest';
	import { sync } from '$lib/hk-sync.svelte';
	import { snapshots, type RoomChoice } from '$lib/hk-snapshot';
	import {
		fetchBoard,
		fetchIssuePhotos,
		hkKey,
		issuePhotosKey,
		issuesKey,
		MAX_ISSUE_PHOTOS,
		photoControl,
		reportIssue,
		type Severity
	} from '$lib/housekeeping';
	import { can, fetchMe, ownerOf } from '$lib/session';
	import IssuePhotos from '$lib/components/IssuePhotos.svelte';
	import MediaUploader from '$lib/components/MediaUploader.svelte';

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

	/** Photos chosen for the report being written; they go up once it has made the issue. */
	let chosen = $state<File[]>([]);
	const control = $derived(photoControl(sync.online, chosen.length));
	/** The issues reported from this screen while online, newest first, each with its photos' uploads. */
	let reported = $state<{ id: string; number: string; files: File[] }[]>([]);
	const photos = createQuery(() => ({
		queryKey: issuePhotosKey(propertyId),
		queryFn: ({ signal }) => fetchIssuePhotos(propertyId, signal),
		enabled: !!propertyId && reported.length > 0 && sync.online
	}));

	/** Uploads still on their way, by issue; leaving the page would drop them. */
	const uploading = $state<Record<string, number>>({});
	const inFlight = $derived(Object.values(uploading).reduce((sum, count) => sum + count, 0));
	const LEAVE_WARNING = 'Photos are still uploading. Leave and drop them?';

	// Leaving within the app asks first; closing or reloading the tab gets the browser's own prompt.
	beforeNavigate((navigation) => {
		if (inFlight > 0 && navigation.type !== 'leave' && !confirm(LEAVE_WARNING)) navigation.cancel();
	});
	$effect(() => {
		if (inFlight === 0) return;
		const guard = (event: BeforeUnloadEvent) => event.preventDefault();
		window.addEventListener('beforeunload', guard);
		return () => window.removeEventListener('beforeunload', guard);
	});

	function choose(files: File[]) {
		chosen = [...chosen, ...files].slice(0, MAX_ISSUE_PHOTOS);
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		sent = '';
		const text = description.trim();
		const number = choices.find((room) => room.roomId === roomId)?.number ?? '';
		const key = keys.keyFor({ roomId, severity, text });
		const files = chosen;
		const notSent =
			files.length > 0 ? ' Its photos were not sent: add them from the board once online.' : '';
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
				sent = `Issue for room ${number} saved on this device; it is sent when you are back online.${notSent}`;
			} else {
				try {
					const issue = await reportIssue(propertyId, roomId, severity, text, key);
					sent = `Issue reported for room ${number}.`;
					reported = [{ id: issue.id, number, files }, ...reported];
				} catch (err) {
					if (err instanceof ApiError) throw err;
					await queue();
					sent = `Issue for room ${number} saved on this device; it is sent when you are back online.${notSent}`;
				}
			}
			description = '';
			chosen = [];
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
		<fieldset class="photos">
			<legend>Photos (up to {MAX_ISSUE_PHOTOS})</legend>
			{#if chosen.length > 0}
				<ul class="chosen" aria-label="Chosen photos">
					{#each chosen as file, index (index)}
						<li>
							<span>{file.name}</span>
							<button
								type="button"
								class="secondary"
								aria-label="Leave out {file.name}"
								onclick={() => (chosen = chosen.filter((_, i) => i !== index))}>Leave out</button
							>
						</li>
					{/each}
				</ul>
			{/if}
			<MediaUploader
				purpose="issue_photo"
				label="Add photos"
				capture="environment"
				disabled={busy || control.disabled}
				onfiles={choose}
			/>
			{#if control.note}<p class="hint">{control.note}</p>{/if}
		</fieldset>
		<button type="submit" disabled={busy || !roomId || !description.trim()}>Report issue</button>
	</form>

	{#each reported as issue (issue.id)}
		<section class="reported" aria-label="Photos of the issue in room {issue.number}">
			<h2>Photos · room {issue.number}</h2>
			<IssuePhotos
				{propertyId}
				issueId={issue.id}
				photos={photos.data?.[issue.id] ?? []}
				mayAttach={mayReport}
				online={sync.online}
				camera
				initial={issue.files}
				onuploading={(count) => (uploading[issue.id] = count)}
			/>
		</section>
	{/each}
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
	.photos {
		display: grid;
		gap: 0.5rem;
		margin: 0;
		padding: 0.5rem 0.75rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.chosen {
		display: grid;
		gap: 0.25rem;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.chosen li {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem;
	}
	.chosen span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.hint {
		margin: 0;
	}
	.reported {
		margin-top: 1.5rem;
	}
	.reported h2 {
		font-size: 1.1rem;
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
