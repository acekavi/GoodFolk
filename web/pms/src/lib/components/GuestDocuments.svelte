<!--
	A guest's ID documents at the property: kind, date and uploader, with upload (a kind, then the
	file), view and delete. Rendered only for `GuestDocumentsManage`. Collapsed at first: the list is
	read only once it is opened, so a reservation with many guests does not query them all. A view fetches a fresh 5-minute
	link each time (each is audited) and never keeps it: an image opens at its `md` size in a dialog, a
	PDF in a new tab, or, if the browser blocks the tab, as a link in the dialog. The list shows no
	thumbnails, since each would cost an audited link.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { onDestroy } from 'svelte';
	import { errorMessage } from '$lib/api/problem';
	import {
		deleteGuestDocument,
		documentKindLabel,
		documentLink,
		DOCUMENT_KINDS,
		fetchGuestDocuments,
		guestDocumentsKey,
		uploaderLabel,
		viewSize,
		type GuestDocument
	} from '$lib/guest-documents';
	import { Pending } from '$lib/pending.svelte';
	import type { DocumentKind } from '$lib/upload';
	import { percent, PROCESSING_FAILED, Uploads } from '$lib/uploads.svelte';
	import MediaUploader from './MediaUploader.svelte';

	interface Props {
		propertyId: string;
		guestId: string;
		/** Whose documents, as the heading names them, such as the guest's name. */
		guestName: string;
	}

	let { propertyId, guestId, guestName }: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	let expanded = $state(false);
	const documents = createQuery(() => ({
		queryKey: guestDocumentsKey(propertyId, guestId),
		queryFn: ({ signal }) => fetchGuestDocuments(propertyId, guestId, signal),
		enabled: expanded
	}));

	// The list cannot tell which row an upload made, but its row exists once the upload completed, so
	// the upload's line goes once the list has been refetched.
	const uploads = new Uploads(async (upload) => {
		await client.invalidateQueries({ queryKey: guestDocumentsKey(propertyId, guestId) });
		uploads.dismiss(upload);
	});
	onDestroy(() => uploads.destroy());

	let kind = $state<DocumentKind | ''>('');
	let problem = $state('');
	let deleting = $state<string | null>(null);
	/** The document being viewed and its link, kept only while the dialog is open. */
	let viewing = $state<{ document: GuestDocument; url: string; pdf: boolean } | null>(null);
	let viewer = $state<HTMLDialogElement>();

	function addFiles(files: File[]) {
		if (!kind) return;
		uploads.add(files, { propertyId, purpose: 'guest_document', targetId: guestId, kind });
	}

	function day(at: string): string {
		return new Date(at).toLocaleDateString(undefined, { dateStyle: 'medium' });
	}

	async function view(document: GuestDocument) {
		problem = '';
		const size = viewSize(document.mime);
		try {
			const { url } = await pending.run(document.id, () =>
				documentLink(propertyId, document.id, size)
			);
			if (size === 'original') {
				const tab = window.open(url, '_blank');
				if (tab) {
					// The file must not reach back into the app.
					tab.opener = null;
					return;
				}
			}
			viewing = { document, url, pdf: size === 'original' };
			viewer?.showModal();
		} catch (err) {
			problem = errorMessage(err);
		}
	}

	async function remove(id: string) {
		problem = '';
		try {
			await pending.run(id, () => deleteGuestDocument(propertyId, id));
			deleting = null;
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: guestDocumentsKey(propertyId, guestId) });
		}
	}
</script>

{#snippet icon()}
	<svg class="icon" viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
		<path
			d="M6 2h8l6 6v14H6z M14 2v6h6 M9 13h8 M9 17h8"
			fill="none"
			stroke="currentColor"
			stroke-width="1.5"
			stroke-linejoin="round"
		/>
	</svg>
{/snippet}

<details class="documents" bind:open={expanded}>
	<summary>ID documents · {guestName}</summary>
	<section aria-label="ID documents of {guestName}">
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}

		{#if documents.isError && !documents.data}
			<p class="error" role="alert">{errorMessage(documents.error)}</p>
			<button type="button" onclick={() => documents.refetch()}>Retry</button>
		{:else if !documents.data}
			<p>Loading…</p>
		{:else}
			{#if documents.data.length === 0 && uploads.items.length === 0}
				<p class="hint">No ID documents on file.</p>
			{/if}
			<ul class="rows" aria-label="ID documents">
				{#each documents.data as document (document.id)}
					<li>
						{@render icon()}
						<span class="what">
							<strong>{documentKindLabel(document.kind)}</strong>
							<span class="hint">
								{day(document.uploadedAt)} · {uploaderLabel(document.uploadedByName)}
							</span>
						</span>
						{#if deleting === document.id}
							<span class="actions">
								<span>Delete this document?</span>
								<button
									type="button"
									disabled={pending.has(document.id)}
									onclick={() => remove(document.id)}>Delete document</button
								>
								<button type="button" class="secondary" onclick={() => (deleting = null)}
									>Keep it</button
								>
							</span>
						{:else}
							<span class="actions">
								{#if document.status === 'READY'}
									<button
										type="button"
										class="secondary"
										aria-label="View {documentKindLabel(document.kind)} from {day(
											document.uploadedAt
										)}"
										disabled={pending.has(document.id)}
										onclick={() => view(document)}>View</button
									>
								{:else if document.status === 'FAILED'}
									<span class="error">{PROCESSING_FAILED}</span>
								{:else}
									<span class="hint">Processing…</span>
								{/if}
								<button
									type="button"
									class="secondary"
									aria-label="Delete {documentKindLabel(document.kind)} from {day(
										document.uploadedAt
									)}"
									disabled={pending.has(document.id)}
									onclick={() => (deleting = document.id)}>Delete</button
								>
							</span>
						{/if}
					</li>
				{/each}
				{#each uploads.items as upload (upload.id)}
					<li aria-label="Upload of {upload.name}">
						{@render icon()}
						<span class="what">
							<strong>{documentKindLabel(upload.target.kind ?? 'other')}</strong>
							<span class="hint name">{upload.name}</span>
						</span>
						<span class="actions">
							{#if upload.state === 'failed'}
								<span class="error" role="alert">{upload.error}</span>
								<button type="button" class="secondary" onclick={() => uploads.dismiss(upload)}
									>Dismiss</button
								>
							{:else}
								{#if upload.state === 'processing'}
									<span class="hint">Processing…</span>
								{:else if upload.state === 'waiting' || !upload.progress || upload.progress.phase === 'hashing'}
									<span class="hint">Waiting…</span>
								{:else}
									<progress max="100" value={percent(upload.progress)}></progress>
									<span class="hint">
										{upload.progress.phase === 'completing'
											? 'Finishing…'
											: `${percent(upload.progress)}%`}
									</span>
								{/if}
								{#if uploads.cancellable(upload)}
									<button type="button" class="secondary" onclick={() => uploads.cancel(upload)}
										>Cancel</button
									>
								{/if}
							{/if}
						</span>
					</li>
				{/each}
			</ul>

			<div class="add">
				<label>
					Kind
					<select bind:value={kind}>
						<option value="" disabled>Choose a kind</option>
						{#each DOCUMENT_KINDS as value (value)}
							<option {value}>{documentKindLabel(value)}</option>
						{/each}
					</select>
				</label>
				<MediaUploader
					purpose="guest_document"
					label="Add document"
					disabled={!kind}
					onfiles={addFiles}
				/>
			</div>
			<p class="hint">Images or PDF, up to 15 MB. Choose the kind first.</p>
		{/if}
	</section>
</details>

<!-- Clicking the backdrop closes the document; from the keyboard, Escape does. -->
<dialog
	class="viewer"
	bind:this={viewer}
	aria-label={viewing ? documentKindLabel(viewing.document.kind) : 'Document'}
	onclose={() => (viewing = null)}
	onclick={(event) => {
		if (event.target === viewer) viewer.close();
	}}
>
	{#if viewing}
		{#if viewing.pdf}
			<p>The browser blocked the new tab.</p>
			<p>
				<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- a signed storage URL, not an app route -->
				<a href={viewing.url} target="_blank" rel="noopener noreferrer"
					>Open the {documentKindLabel(viewing.document.kind)} (PDF)</a
				>
			</p>
			<p class="hint">The link works for 5 minutes.</p>
		{:else}
			<img src={viewing.url} alt="{documentKindLabel(viewing.document.kind)} of {guestName}" />
		{/if}
		<button type="button" class="secondary" onclick={() => viewer?.close()}>Close</button>
	{/if}
</dialog>

<style>
	.documents {
		margin: 0.5rem 0;
	}
	summary {
		cursor: pointer;
		font-weight: 600;
		padding: 0.25rem 0;
	}
	section {
		padding: 0.5rem 0 0 0.5rem;
	}
	.rows {
		list-style: none;
		margin: 0 0 var(--space);
		padding: 0;
		display: grid;
		gap: 0.25rem;
	}
	.rows li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
		padding: 0.35rem 0;
		border-bottom: 1px solid var(--border);
	}
	.icon {
		flex: none;
		color: var(--muted);
	}
	.what {
		flex: 1;
		min-width: 10rem;
		display: grid;
	}
	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
	}
	.add {
		display: flex;
		flex-wrap: wrap;
		align-items: end;
		gap: var(--space);
	}
	.add label {
		display: grid;
		gap: 0.25rem;
	}
	.hint {
		margin: 0;
	}
	.viewer {
		width: min(1024px, calc(100vw - 2rem));
		max-height: calc(100vh - 2rem);
		padding: 0.75rem;
	}
	.viewer::backdrop {
		background: rgb(0 0 0 / 0.6);
	}
	.viewer img {
		display: block;
		max-width: 100%;
		max-height: calc(100vh - 7rem);
		margin: 0 auto;
		object-fit: contain;
	}
	.viewer button {
		margin-top: 0.5rem;
	}
</style>
