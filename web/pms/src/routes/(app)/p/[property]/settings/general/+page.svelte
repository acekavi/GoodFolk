<!--
	The property's general settings: its name, check-in and check-out times, and the audit time, changed with
	`PropertiesManage` and `If-Match` on the property. The time zone and base currency are shown read-only: changing
	either would move the business date and every stored base amount.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((p) => p.id === propertyId));
	const manage = $derived(!!me.data && can(me.data, 'propertiesManage', propertyId));

	type Property = NonNullable<typeof property>;

	function draftOf(property: Property) {
		return {
			name: property.name,
			checkIn: property.checkInTime,
			checkOut: property.checkOutTime,
			audit: property.auditTime
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
		if (!property || property.version === draftVersion) return;
		const dirty =
			draftVersion !== -1 &&
			!!draft &&
			JSON.stringify($state.snapshot(draft)) !== JSON.stringify(stored);
		if (dirty) return;
		draftVersion = property.version;
		stored = draftOf(property);
		draft = draftOf(property);
	});

	const pending = new Pending();
	let problem = $state('');
	let saved = $state(false);

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!draft || draftVersion === -1) return;
		problem = '';
		saved = false;
		const body = {
			name: draft.name.trim(),
			check_in_time: draft.checkIn,
			check_out_time: draft.checkOut,
			audit_time: draft.audit
		};
		try {
			await pending.run('save', async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}', {
						params: { path: { property: propertyId }, header: ifMatch(draftVersion) },
						body
					})
				)
			);
			saved = true;
			stored = $state.snapshot(draft);
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				draftVersion = -1;
				problem =
					'Someone else changed this property. The form now shows the latest version; make your change again.';
			} else {
				problem = errorMessage(err);
			}
		} finally {
			await client.invalidateQueries({ queryKey: propertiesKey });
		}
	}
</script>

<h1>General</h1>
{#if !manage}
	<p class="hint">Only owners and managers can change these settings.</p>
{/if}

{#if properties.error && !properties.data}
	<p class="error" role="alert">{errorMessage(properties.error)}</p>
	<button onclick={() => properties.refetch()}>Retry</button>
{:else if property && draft}
	<form class="form general" aria-label="General settings" onsubmit={save}>
		<fieldset disabled={!manage}>
			<label>Name <input required maxlength="200" bind:value={draft.name} /></label>
			<label>Check-in time <input type="time" required bind:value={draft.checkIn} /></label>
			<label>Check-out time <input type="time" required bind:value={draft.checkOut} /></label>
			<label>
				Audit time
				<input type="time" required bind:value={draft.audit} />
				<span class="hint"
					>When the day closes by itself if nobody has closed it: on the business date at 12:00 or
					later, the next morning when earlier.</span
				>
			</label>
			{#if manage}
				<div class="actions">
					<button disabled={pending.has('save')}>Save</button>
				</div>
			{/if}
		</fieldset>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		{#if saved}<p class="hint" role="status">Saved.</p>{/if}
	</form>
	<dl class="facts">
		<dt>Code</dt>
		<dd>{property.code}</dd>
		<dt>Time zone</dt>
		<dd>{property.timezone}</dd>
		<dt>Base currency</dt>
		<dd>{property.baseCurrency}</dd>
	</dl>
	<p class="hint">
		The time zone and base currency can't be changed here: either would move the business date and
		every amount already stored in the base currency.
	</p>
{:else if properties.isSuccess}
	<p>Property not found.</p>
{:else}
	<p>Loading…</p>
{/if}

<style>
	.general fieldset {
		display: grid;
		gap: var(--space);
		border: none;
		padding: 0;
		margin: 0;
		max-width: 28rem;
	}
	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.25rem 1rem;
	}
	.facts dt {
		color: var(--muted);
	}
	.facts dd {
		margin: 0;
	}
</style>
