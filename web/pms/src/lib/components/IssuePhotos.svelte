<!--
	A room issue's photos: thumbnails that open the large size in a dialog, and, with `mayAttach`
	(`RoomIssuesReport`), uploads of up to five photos and removal. Only the photo's uploader or a
	supervisor may remove one; the server refuses anyone else, and the refusal is shown. Uploads show
	their progress, then "Processing…" until the `media:<property>` event refetches the photos.
	Self-contained: it runs its own commands and refetches the issue photos after each.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { onDestroy, onMount } from 'svelte';
	import {
		issuePhotosKey,
		overPhotoLimit,
		photoControl,
		removeIssuePhoto,
		removePhotoMessage
	} from '$lib/housekeeping';
	import type { MediaItem } from '$lib/media';
	import { Pending } from '$lib/pending.svelte';
	import { percent, PROCESSING_FAILED, Uploads } from '$lib/uploads.svelte';
	import MediaUploader from './MediaUploader.svelte';
	import Picture from './Picture.svelte';

	interface Props {
		propertyId: string;
		issueId: string;
		photos: MediaItem[];
		mayAttach: boolean;
		/** Without a connection nothing can be uploaded. */
		online?: boolean;
		/** Open the phone's camera from the picker. */
		camera?: boolean;
		/** Files chosen before the issue existed, uploaded as soon as this mounts. */
		initial?: File[];
		/** Told how many uploads are still on their way, which leaving would drop, whenever it changes. */
		onuploading?: (count: number) => void;
	}

	let {
		propertyId,
		issueId,
		photos,
		mayAttach,
		online = true,
		camera = false,
		initial = [],
		onuploading
	}: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	const uploads = new Uploads(
		() => void client.invalidateQueries({ queryKey: issuePhotosKey(propertyId) })
	);
	onDestroy(() => uploads.destroy());
	$effect(() => uploads.settle(photos.map((photo) => photo.assetId)));
	$effect(() => onuploading?.(uploads.inFlight));

	const control = $derived(photoControl(online, photos.length + uploads.active));
	let error = $state('');
	let removing = $state<string | null>(null);
	let opened = $state<{ photo: MediaItem; index: number } | null>(null);
	let viewer = $state<HTMLDialogElement>();

	function addFiles(files: File[]) {
		const taken = photos.length + uploads.active;
		uploads.add(files, { propertyId, purpose: 'issue_photo', targetId: issueId }, (_, index) =>
			overPhotoLimit(taken, index)
		);
	}

	onMount(() => {
		if (initial.length > 0) addFiles(initial);
	});

	async function remove(assetId: string) {
		error = '';
		try {
			await pending.run(assetId, () => removeIssuePhoto(propertyId, issueId, assetId));
			removing = null;
		} catch (err) {
			error = removePhotoMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: issuePhotosKey(propertyId) });
		}
	}

	function open(photo: MediaItem, index: number) {
		opened = { photo, index };
		viewer?.showModal();
	}
</script>

{#if photos.length > 0 || uploads.items.length > 0 || mayAttach}
	<div class="issue-photos">
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		{#if photos.length > 0 || uploads.items.length > 0}
			<div class="thumbs">
				{#each photos as photo, index (photo.assetId)}
					<div class="thumb">
						<div class="frame">
							{#if photo.status === 'READY'}
								<button
									type="button"
									class="open"
									aria-label="Open photo {index + 1}"
									onclick={() => open(photo, index)}
								>
									<Picture
										renditions={photo.renditions}
										alt="Photo {index + 1}"
										sizes="5rem"
										variant="thumb"
									/>
								</button>
							{:else if photo.status === 'FAILED'}
								<span class="state error">{photo.error ?? PROCESSING_FAILED}</span>
							{:else}
								<span class="state">Processing…</span>
							{/if}
						</div>
						{#if mayAttach && removing !== photo.assetId}
							<button
								type="button"
								class="secondary small"
								aria-label="Remove photo {index + 1}"
								disabled={pending.has(photo.assetId)}
								onclick={() => (removing = photo.assetId)}>Remove</button
							>
						{/if}
						{#if removing === photo.assetId}
							<button
								type="button"
								class="small"
								disabled={pending.has(photo.assetId)}
								onclick={() => remove(photo.assetId)}>Remove photo</button
							>
							<button type="button" class="secondary small" onclick={() => (removing = null)}
								>Keep it</button
							>
						{/if}
					</div>
				{/each}

				{#each uploads.items as upload (upload.id)}
					<div class="thumb" role="group" aria-label="Upload of {upload.name}">
						<div class="frame">
							{#if upload.state === 'failed'}
								<span class="state error" role="alert">{upload.error}</span>
							{:else if upload.state === 'processing'}
								<span class="state">Processing…</span>
							{:else if upload.state === 'waiting' || !upload.progress || upload.progress.phase === 'hashing'}
								<span class="state">Waiting…</span>
							{:else}
								<span class="state">
									<progress max="100" value={percent(upload.progress)}></progress>
									{upload.progress.phase === 'completing'
										? 'Finishing…'
										: `${percent(upload.progress)}%`}
								</span>
							{/if}
						</div>
						{#if uploads.cancellable(upload)}
							<button type="button" class="secondary small" onclick={() => uploads.cancel(upload)}
								>Cancel</button
							>
						{:else if upload.state === 'failed'}
							<button type="button" class="secondary small" onclick={() => uploads.dismiss(upload)}
								>Dismiss</button
							>
						{/if}
					</div>
				{/each}
			</div>
		{/if}

		{#if mayAttach}
			<MediaUploader
				purpose="issue_photo"
				label="Add photos"
				capture={camera ? 'environment' : undefined}
				disabled={control.disabled}
				onfiles={addFiles}
			/>
			{#if control.note}<p class="hint">{control.note}</p>{/if}
		{/if}
	</div>
{/if}

<!-- Clicking the backdrop closes the photo; from the keyboard, Escape does. -->
<dialog
	class="viewer"
	bind:this={viewer}
	aria-label={opened ? `Photo ${opened.index + 1}` : 'Photo'}
	onclose={() => (opened = null)}
	onclick={(event) => {
		if (event.target === viewer) viewer.close();
	}}
>
	{#if opened}
		<Picture
			renditions={opened.photo.renditions}
			alt="Photo {opened.index + 1}"
			sizes="100vw"
			variant="large"
		/>
		<button type="button" class="secondary" onclick={() => viewer?.close()}>Close</button>
	{/if}
</dialog>

<style>
	.issue-photos {
		display: grid;
		gap: 0.4rem;
		flex-basis: 100%;
	}
	.thumbs {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}
	.thumb {
		display: grid;
		gap: 0.25rem;
		align-content: start;
		width: 5rem;
	}
	.frame {
		aspect-ratio: 1;
		overflow: hidden;
		border-radius: var(--radius);
		background: var(--surface);
		display: grid;
		place-items: center;
	}
	.frame :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.open {
		all: unset;
		display: block;
		width: 100%;
		height: 100%;
		cursor: zoom-in;
	}
	.open:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
	.state {
		padding: 0.25rem;
		font-size: 0.75rem;
		text-align: center;
		color: var(--muted);
		overflow-wrap: anywhere;
		display: grid;
		gap: 0.25rem;
		justify-items: center;
	}
	.state.error {
		color: var(--danger);
	}
	progress {
		width: 100%;
	}
	.small {
		padding: 0.15rem 0.4rem;
		font-size: 0.8rem;
	}
	.hint {
		margin: 0;
	}
	.viewer {
		width: min(1600px, calc(100vw - 2rem));
		max-height: calc(100vh - 2rem);
		padding: 0.75rem;
	}
	.viewer::backdrop {
		background: rgb(0 0 0 / 0.6);
	}
	.viewer :global(img) {
		max-height: calc(100vh - 7rem);
		object-fit: contain;
	}
	.viewer button {
		margin-top: 0.5rem;
	}
</style>
