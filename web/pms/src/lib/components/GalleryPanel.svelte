<!--
	A room type's Photos panel. Anyone who sees room types sees the gallery; with `manage`
	(`MediaManage`) photos can also be uploaded, dragged into a new order (or moved with the arrows),
	captioned and removed. Uploads show as tiles with their progress, then "Processing…" until the
	`media:<property>` event refetches the gallery with the finished photo. Self-contained: it runs its
	own commands and refetches the galleries after each.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { onDestroy } from 'svelte';
	import { errorMessage } from '$lib/api/problem';
	import { rest, unwrap } from '$lib/api/rest';
	import { dropIndex, mediaKey, type Box, type MediaItem } from '$lib/media';
	import { moveItem } from '$lib/rooms';
	import { percent, PROCESSING_FAILED, Uploads } from '$lib/uploads.svelte';
	import MediaUploader from './MediaUploader.svelte';
	import Picture from './Picture.svelte';

	interface Props {
		propertyId: string;
		roomType: { id: string; code: string; name: string };
		media: MediaItem[];
		manage: boolean;
	}

	let { propertyId, roomType, media, manage }: Props = $props();

	const client = useQueryClient();
	const CAPTION_MAX = 200;
	/** A pointer-down becomes a drag once it has moved this far (px); less is a click. */
	const DRAG_THRESHOLD = 6;

	const uploads = new Uploads(
		() => void client.invalidateQueries({ queryKey: mediaKey(propertyId) })
	);
	// Leaving the page stops its uploads.
	onDestroy(() => uploads.destroy());
	// A finished upload's tile goes once the gallery shows its photo.
	$effect(() => uploads.settle(media.map((item) => item.assetId)));

	let error = $state('');
	let busy = $state(false);
	let captioning = $state<{ assetId: string; caption: string } | null>(null);
	let removing = $state<string | null>(null);

	function addFiles(files: File[]) {
		uploads.add(files, { propertyId, purpose: 'gallery', targetId: roomType.id });
	}

	/** Runs a command, shows its problem if it fails, and refetches the galleries either way. */
	async function run(command: () => Promise<unknown>, onError?: () => void) {
		busy = true;
		error = '';
		try {
			await command();
		} catch (err) {
			error = errorMessage(err);
			onError?.();
		} finally {
			busy = false;
			await client.invalidateQueries({ queryKey: mediaKey(propertyId) });
		}
	}

	function setGallery(items: MediaItem[]) {
		client.setQueryData<Record<string, MediaItem[]>>(mediaKey(propertyId), (galleries) =>
			galleries ? { ...galleries, [roomType.id]: items } : galleries
		);
	}

	function reorder(from: number, to: number) {
		const current = media;
		const moved = moveItem(current, from, to);
		if (moved.every((item, index) => item.assetId === current[index].assetId)) return;
		// Show the new order at once; the refetch after the command confirms it.
		setGallery(moved);
		return run(
			async () =>
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/room-types/{room_type}/media/order', {
						params: { path: { property: propertyId, room_type: roomType.id } },
						body: { asset_ids: moved.map((item) => item.assetId) }
					})
				),
			() => setGallery(current)
		);
	}

	function saveCaption(event: SubmitEvent) {
		event.preventDefault();
		const editing = captioning;
		if (!editing) return;
		return run(async () => {
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/room-types/{room_type}/media/{asset}', {
					params: {
						path: { property: propertyId, room_type: roomType.id, asset: editing.assetId }
					},
					body: { caption: editing.caption.trim() }
				})
			);
			captioning = null;
		});
	}

	function remove(assetId: string) {
		return run(async () => {
			unwrap(
				await rest.DELETE('/api/v1/properties/{property}/room-types/{room_type}/media/{asset}', {
					params: { path: { property: propertyId, room_type: roomType.id, asset: assetId } }
				})
			);
			removing = null;
		});
	}

	// Drag to reorder, with pointer events as the tape chart does: the tiles' boxes are read once when
	// a drag begins, and each move is only arithmetic.
	interface Grab {
		from: number;
		pointerId: number;
		startX: number;
		startY: number;
		boxes: Box[];
		dragging: boolean;
	}
	const tileEls: HTMLElement[] = $state([]);
	let grab: Grab | undefined;
	let dragFrom = $state<number | null>(null);
	let dropAt = $state<number | null>(null);

	function grabTile(event: PointerEvent & { currentTarget: HTMLElement }, index: number) {
		if (!manage || busy || event.button !== 0) return;
		event.currentTarget.setPointerCapture(event.pointerId);
		grab = {
			from: index,
			pointerId: event.pointerId,
			startX: event.clientX,
			startY: event.clientY,
			boxes: [],
			dragging: false
		};
	}

	function dragMove(event: PointerEvent) {
		const g = grab;
		if (!g || event.pointerId !== g.pointerId) return;
		if (!g.dragging) {
			if (Math.hypot(event.clientX - g.startX, event.clientY - g.startY) < DRAG_THRESHOLD) return;
			g.dragging = true;
			g.boxes = tileEls.slice(0, media.length).map((el) => el.getBoundingClientRect());
			dragFrom = g.from;
		}
		dropAt = dropIndex(g.boxes, event.clientX, event.clientY);
	}

	function dragEnd(event: PointerEvent) {
		const g = grab;
		if (!g || event.pointerId !== g.pointerId) return;
		const to = dropAt;
		dragCancel();
		if (g.dragging && to !== null && to >= 0) void reorder(g.from, to);
	}

	function dragCancel() {
		grab = undefined;
		dragFrom = null;
		dropAt = null;
	}
</script>

<section class="gallery" aria-label="Photos of {roomType.code}">
	<h3>{roomType.code} · {roomType.name}</h3>
	{#if error}<p class="error" role="alert">{error}</p>{/if}

	{#if media.length === 0 && uploads.items.length === 0}
		<p class="hint">No photos yet.</p>
	{/if}

	<ul class="tiles">
		{#each media as item, index (item.assetId)}
			<li
				bind:this={tileEls[index]}
				class="tile"
				class:dragging={dragFrom === index}
				class:target={dropAt === index && dragFrom !== index}
			>
				<div class="frame">
					{#if item.status === 'READY'}
						<Picture
							renditions={item.renditions}
							alt={item.caption ?? `Photo ${index + 1} of ${roomType.code}`}
							sizes="12rem"
							variant="thumb"
						/>
					{:else if item.status === 'FAILED'}
						<p class="state error">{item.error ?? PROCESSING_FAILED}</p>
					{:else}
						<p class="state">Processing…</p>
					{/if}
					{#if manage}
						<button
							type="button"
							class="secondary handle"
							aria-label="Drag photo {index + 1} of {roomType.code}"
							title="Drag to reorder"
							tabindex="-1"
							disabled={busy}
							onpointerdown={(event) => grabTile(event, index)}
							onpointermove={dragMove}
							onpointerup={dragEnd}
							onpointercancel={dragCancel}>⠿</button
						>
					{/if}
				</div>

				{#if captioning?.assetId === item.assetId}
					<form class="caption-form" aria-label="Caption" onsubmit={saveCaption}>
						<input
							aria-label="Caption of photo {index + 1}"
							maxlength={CAPTION_MAX}
							bind:value={captioning.caption}
						/>
						<div class="actions">
							<button disabled={busy}>Save</button>
							<button type="button" class="secondary" onclick={() => (captioning = null)}
								>Cancel</button
							>
						</div>
					</form>
				{:else if removing === item.assetId}
					<div class="confirm">
						<p>Remove this photo from the gallery?</p>
						<div class="actions">
							<button disabled={busy} onclick={() => remove(item.assetId)}>Remove photo</button>
							<button type="button" class="secondary" onclick={() => (removing = null)}
								>Keep it</button
							>
						</div>
					</div>
				{:else}
					{#if item.caption}<p class="caption">{item.caption}</p>{/if}
					{#if manage}
						<div class="actions">
							<button
								type="button"
								class="secondary"
								aria-label="Move photo {index + 1} earlier"
								disabled={busy || index === 0}
								onclick={() => reorder(index, index - 1)}>←</button
							>
							<button
								type="button"
								class="secondary"
								aria-label="Move photo {index + 1} later"
								disabled={busy || index === media.length - 1}
								onclick={() => reorder(index, index + 1)}>→</button
							>
							<button
								type="button"
								class="secondary"
								aria-label="Edit caption of photo {index + 1}"
								disabled={busy}
								onclick={() => {
									removing = null;
									captioning = { assetId: item.assetId, caption: item.caption ?? '' };
								}}>Caption</button
							>
							<button
								type="button"
								class="secondary"
								aria-label="Remove photo {index + 1}"
								disabled={busy}
								onclick={() => {
									captioning = null;
									removing = item.assetId;
								}}>Remove</button
							>
						</div>
					{/if}
				{/if}
			</li>
		{/each}

		{#each uploads.items as upload (upload.id)}
			<li class="tile upload" aria-label="Upload of {upload.name}">
				<div class="frame">
					{#if upload.state === 'failed'}
						<p class="state error" role="alert">{upload.error}</p>
					{:else if upload.state === 'processing'}
						<p class="state">Processing…</p>
					{:else if upload.state === 'waiting' || !upload.progress || upload.progress.phase === 'hashing'}
						<p class="state">Waiting…</p>
					{:else}
						<p class="state">
							<progress max="100" value={percent(upload.progress)}></progress>
							{upload.progress.phase === 'completing'
								? 'Finishing…'
								: `${percent(upload.progress)}%`}
						</p>
					{/if}
				</div>
				<p class="caption" title={upload.name}>{upload.name}</p>
				<div class="actions">
					{#if uploads.cancellable(upload)}
						<button type="button" class="secondary" onclick={() => uploads.cancel(upload)}
							>Cancel</button
						>
					{:else if upload.state === 'failed'}
						<button type="button" class="secondary" onclick={() => uploads.dismiss(upload)}
							>Dismiss</button
						>
					{/if}
				</div>
			</li>
		{/each}
	</ul>

	{#if manage}
		<MediaUploader purpose="gallery" label="Add photos" onfiles={addFiles} />
	{/if}
</section>

<style>
	.gallery {
		margin: var(--space) 0;
		padding: 0 var(--space) var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.tiles {
		list-style: none;
		padding: 0;
		margin: 0 0 var(--space);
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(10rem, 100%), 1fr));
		gap: var(--space);
	}
	.tile {
		display: grid;
		gap: 0.35rem;
		align-content: start;
		min-width: 0;
	}
	.tile.dragging {
		opacity: 0.5;
	}
	.tile.target .frame {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.frame {
		position: relative;
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
	.state {
		margin: 0;
		padding: 0.5rem;
		text-align: center;
		color: var(--muted);
		display: grid;
		gap: 0.35rem;
		justify-items: center;
	}
	.state.error {
		color: var(--danger);
	}
	progress {
		width: 100%;
	}
	.handle {
		position: absolute;
		top: 0.35rem;
		right: 0.35rem;
		padding: 0.2rem 0.45rem;
		cursor: grab;
		/* The handle drags on touch screens; the rest of the tile still scrolls the page. */
		touch-action: none;
	}
	.caption {
		margin: 0;
		color: var(--muted);
		overflow-wrap: anywhere;
	}
	.upload .caption {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.actions {
		flex-wrap: wrap;
	}
	.caption-form,
	.confirm {
		display: grid;
		gap: 0.35rem;
	}
	.confirm p {
		margin: 0;
	}
	.caption-form input {
		width: 100%;
	}
</style>
