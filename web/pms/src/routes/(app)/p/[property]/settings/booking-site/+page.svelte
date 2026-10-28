<!--
	The property's Booking site settings: whether the public site is on, its address, logo, hero image, accent colour,
	description, contact details, and the terms and privacy notice the guest agrees to. Owners and managers
	(`IbeManage`) change them, with `If-Match` on the site's version. The logo is uploaded straight away and saved
	with the rest; the hero image is chosen from the room type galleries.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { onDestroy } from 'svelte';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import MediaUploader from '$lib/components/MediaUploader.svelte';
	import Picture from '$lib/components/Picture.svelte';
	import {
		accentProblem,
		contrastWithWhite,
		fetchIbeSite,
		IBE_PUBLIC_ORIGIN,
		ibeSiteKey,
		previewUrl
	} from '$lib/ibe';
	import { fetchGalleries, mediaKey, type MediaItem } from '$lib/media';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchRatePlans, ratePlansKey } from '$lib/rates';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';
	import { uploadErrorMessage, uploadFile } from '$lib/upload';

	const DESCRIPTION_MAX = 500;
	const TEXT_MAX = 10_000;

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'ibeManage', propertyId));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const code = $derived(properties.data?.find((p) => p.id === propertyId)?.code ?? '');
	const site = createQuery(() => ({
		queryKey: ibeSiteKey(propertyId),
		queryFn: ({ signal }) => fetchIbeSite(propertyId, signal)
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const galleries = createQuery(() => ({
		queryKey: mediaKey(propertyId),
		queryFn: ({ signal }) => fetchGalleries(propertyId, signal)
	}));
	const plans = createQuery(() => ({
		queryKey: ratePlansKey(propertyId),
		queryFn: ({ signal }) => fetchRatePlans(propertyId, signal)
	}));

	type Site = NonNullable<typeof site.data>;

	function draftOf(site: Site) {
		return {
			enabled: site.enabled,
			customHost: site.custom_host ?? '',
			logoAssetId: site.logo_asset_id ?? null,
			heroAssetId: site.hero_asset_id ?? null,
			accent: site.accent ?? '',
			description: site.description,
			contactPhone: site.contact_phone ?? '',
			contactEmail: site.contact_email ?? '',
			terms: site.terms,
			privacy: site.privacy
		};
	}

	type Draft = ReturnType<typeof draftOf>;
	let draft = $state<Draft | null>(null);
	/** The stored settings the draft was taken from, and their version, which its save sends as `If-Match`. */
	let stored: Draft | null = null;
	let draftVersion = $state(-1);
	// As on the Taxes page: a new version replaces the form only while it still holds what was stored (or after a
	// 412 resets `draftVersion`), so an edit in progress is kept and its save is refused if someone else saved first.
	$effect(() => {
		const current = site.data;
		if (!current || current.version === draftVersion) return;
		const dirty =
			draftVersion !== -1 &&
			!!draft &&
			JSON.stringify($state.snapshot(draft)) !== JSON.stringify(stored);
		if (dirty) return;
		draftVersion = current.version;
		stored = draftOf(current);
		draft = draftOf(current);
	});

	const pending = new Pending();
	let problem = $state('');
	let saved = $state(false);
	let logoProblem = $state('');
	const upload = new AbortController();
	onDestroy(() => upload.abort());

	const accent = $derived(draft?.accent.trim() ?? '');
	const accentIssue = $derived(accent ? accentProblem(accent) : null);
	const contrast = $derived(accent ? contrastWithWhite(accent) : null);
	const sellsOnline = $derived(
		(plans.data?.ratePlans ?? []).some((plan) => plan.segment === 'IBE' && plan.active)
	);
	const preview = $derived(code ? previewUrl(code) : null);
	const bookingDomain = $derived(IBE_PUBLIC_ORIGIN ? new URL(IBE_PUBLIC_ORIGIN).host : '');

	/** The ready gallery photos, with the room type each is in. */
	const photos = $derived.by(() => {
		const rows: { item: MediaItem; roomType: string }[] = [];
		for (const type of roomTypes.data ?? []) {
			for (const item of galleries.data?.[type.id] ?? []) {
				if (item.kind === 'IMAGE' && item.status === 'READY')
					rows.push({ item, roomType: type.name });
			}
		}
		return rows;
	});

	/** The logo shown: the saved one's URL while the draft still holds it. */
	const logoUrl = $derived(
		draft && draft.logoAssetId === site.data?.logo_asset_id ? site.data?.logo_url : null
	);
	const logoUnsaved = $derived(
		!!draft?.logoAssetId && draft.logoAssetId !== site.data?.logo_asset_id
	);
	const logoFailed = $derived(!logoUnsaved && site.data?.logo_status === 'failed');

	async function addLogo(files: File[]) {
		if (!draft || files.length === 0) return;
		logoProblem = '';
		try {
			const asset = await pending.run('logo', () =>
				uploadFile(files[0], {
					propertyId,
					purpose: 'ibe_logo',
					targetId: propertyId,
					signal: upload.signal
				})
			);
			if (asset.status === 'failed') logoProblem = asset.error ?? 'Processing failed';
			else draft.logoAssetId = asset.asset_id;
		} catch (err) {
			if (!upload.signal.aborted) logoProblem = uploadErrorMessage(err);
		}
	}

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!draft || draftVersion === -1) return;
		problem = '';
		saved = false;
		const body = {
			enabled: draft.enabled,
			custom_host: draft.customHost.trim().toLowerCase() || null,
			logo_asset_id: draft.logoAssetId,
			hero_asset_id: draft.heroAssetId,
			accent: accent.toLowerCase() || null,
			description: draft.description.trim(),
			contact_phone: draft.contactPhone.trim() || null,
			contact_email: draft.contactEmail.trim() || null,
			terms: draft.terms.trim(),
			privacy: draft.privacy.trim()
		};
		try {
			await pending.run('save', async () =>
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/ibe-site', {
						params: { path: { property: propertyId }, header: ifMatch(draftVersion) },
						body
					})
				)
			);
			saved = true;
			// The form holds what was saved, so it takes the saved settings' new version when it arrives.
			stored = $state.snapshot(draft);
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				draftVersion = -1;
				problem =
					'Someone else changed these settings. The form now shows the latest version; make your change again.';
			} else {
				problem = errorMessage(err);
			}
		} finally {
			await client.invalidateQueries({ queryKey: ibeSiteKey(propertyId) });
		}
	}
</script>

<h1>Booking site</h1>
{#if !manage}
	<p class="hint">Only owners and managers can change these settings.</p>
{/if}

{#if site.error && !site.data}
	<p class="error" role="alert">{errorMessage(site.error)}</p>
	<button onclick={() => site.refetch()}>Retry</button>
{:else if draft}
	{#if plans.isSuccess && !sellsOnline}
		<p class="note" role="status">
			No rate plans are sold on the booking site yet: give a plan the IBE segment.
		</p>
	{/if}
	<form class="form booking-site" aria-label="Booking site settings" onsubmit={save}>
		<fieldset disabled={!manage}>
			<label class="check">
				<input type="checkbox" bind:checked={draft.enabled} />
				Booking site is on
			</label>
			<p class="hint">
				While it is off, the site answers “not found” for this property.
				{#if preview}
					<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- the public site is another app -->
					<a href={preview} target="_blank" rel="noopener noreferrer">Preview</a>
				{/if}
			</p>

			<label>
				Custom host
				<input
					bind:value={draft.customHost}
					maxlength="253"
					pattern={'[a-z0-9.\\-]{3,253}'}
					inputmode="url"
					autocapitalize="none"
					spellcheck="false"
					placeholder="book.example.com"
				/>
				<span class="hint"
					>Optional, lowercase. Point a CNAME at {bookingDomain || 'the booking domain'}.</span
				>
			</label>

			<div class="field">
				<span class="label" id="logo-label">Logo</span>
				<div class="logo" aria-labelledby="logo-label">
					{#if logoUrl}
						<img src={logoUrl} alt="Logo" />
					{:else if logoUnsaved}
						<span class="hint" role="status">Logo uploaded; save to use it.</span>
					{:else if logoFailed}
						<span class="error" role="status"
							>The logo could not be processed; upload it again.</span
						>
					{:else if draft.logoAssetId}
						<span class="hint" role="status">Processing the logo…</span>
					{:else}
						<span class="hint">No logo.</span>
					{/if}
				</div>
				{#if manage}
					<MediaUploader
						purpose="ibe_logo"
						label={draft.logoAssetId ? 'Replace logo' : 'Add logo'}
						disabled={pending.has('logo')}
						onfiles={addLogo}
					/>
					{#if draft.logoAssetId}
						<button type="button" class="secondary" onclick={() => (draft!.logoAssetId = null)}
							>Remove logo</button
						>
					{/if}
				{/if}
				{#if pending.has('logo')}<span class="hint" role="status">Uploading…</span>{/if}
				{#if logoProblem}<p class="error" role="alert">{logoProblem}</p>{/if}
				<span class="hint">A JPEG, PNG or WebP of at most 5 MB.</span>
			</div>

			<fieldset class="hero">
				<legend>Hero image</legend>
				{#if photos.length === 0}
					<p class="hint">Add photos to a room type's gallery to choose one here.</p>
				{:else}
					<div class="choices">
						<label class="choice none">
							<input type="radio" name="hero" value={null} bind:group={draft.heroAssetId} />
							None
						</label>
						{#each photos as { item, roomType } (item.assetId)}
							<label class="choice">
								<input
									type="radio"
									name="hero"
									value={item.assetId}
									bind:group={draft.heroAssetId}
								/>
								<Picture
									renditions={item.renditions}
									alt="{roomType}{item.caption ? `: ${item.caption}` : ''}"
									sizes="8rem"
									variant="thumb"
								/>
							</label>
						{/each}
					</div>
				{/if}
			</fieldset>

			<label>
				Accent colour
				<span class="accent">
					<input
						bind:value={draft.accent}
						maxlength="7"
						placeholder="#1d4ed8"
						autocapitalize="none"
						spellcheck="false"
						aria-invalid={accentIssue ? 'true' : undefined}
						aria-describedby="accent-note"
					/>
					<span
						class="swatch"
						style:background={accent && !accentIssue ? accent : 'var(--muted)'}
						aria-hidden="true">Book now</span
					>
				</span>
				<span id="accent-note" class:error={!!accentIssue} class="hint" role="status">
					{#if accentIssue}
						{accentIssue}
					{:else if contrast}
						Contrast with white text {contrast.toFixed(1)}:1.
					{:else}
						White text on it must reach a contrast of 4.5:1.
					{/if}
				</span>
			</label>

			<label>
				Short description
				<textarea maxlength={DESCRIPTION_MAX} rows="3" bind:value={draft.description}></textarea>
				<span class="hint">{draft.description.length} of {DESCRIPTION_MAX} characters.</span>
			</label>
			<label>
				Contact phone
				<input type="tel" maxlength="50" bind:value={draft.contactPhone} />
			</label>
			<label>
				Contact email
				<input type="email" maxlength="254" bind:value={draft.contactEmail} />
			</label>
			<label>
				Booking terms
				<textarea maxlength={TEXT_MAX} rows="6" bind:value={draft.terms}></textarea>
				<span class="hint">Plain text. The guest agrees to it when booking.</span>
			</label>
			<label>
				Privacy notice
				<textarea maxlength={TEXT_MAX} rows="6" bind:value={draft.privacy}></textarea>
				<span class="hint">Plain text.</span>
			</label>

			{#if manage}
				<div class="actions">
					<button disabled={pending.has('save') || pending.has('logo') || !!accentIssue}
						>Save</button
					>
				</div>
			{/if}
		</fieldset>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		{#if saved}<p class="hint" role="status">Saved.</p>{/if}
	</form>
{:else}
	<p>Loading…</p>
{/if}

<style>
	.booking-site {
		max-width: 36rem;
	}
	.booking-site > fieldset {
		display: grid;
		gap: var(--space);
		border: none;
		padding: 0;
		margin: 0;
	}
	.note {
		max-width: 36rem;
		padding: 0.5rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	label.check {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		color: var(--text);
	}
	.field {
		display: grid;
		gap: 0.25rem;
	}
	.label {
		color: var(--muted);
	}
	.logo img {
		max-height: 4rem;
		max-width: 12rem;
	}
	.hero {
		border: none;
		padding: 0;
		margin: 0;
	}
	.hero legend {
		padding: 0;
		color: var(--muted);
	}
	.choices {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(7rem, 1fr));
		gap: 0.5rem;
	}
	.choice {
		display: grid;
		gap: 0.25rem;
		cursor: pointer;
	}
	.choice :global(img) {
		width: 100%;
		height: auto;
		border-radius: var(--radius);
		border: 2px solid transparent;
	}
	.choice input:checked + :global(picture) :global(img) {
		border-color: var(--accent);
	}
	.accent {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
	}
	.swatch {
		padding: 0.35rem 0.75rem;
		border-radius: var(--radius);
		color: #fff;
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
