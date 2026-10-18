<!--
	The property's POS outlets: their bills' category is `pos:<code>`, so a code never changes once made. Everyone
	with `FolioView` reads them; only `TaxSettingsManage` (the owner) adds or changes them.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { folioContextKey } from '$lib/folio';
	import { Pending } from '$lib/pending.svelte';
	import { can, fetchMe } from '$lib/session';
	import { fetchOutlets, outletsKey, taxSettingsKey, type OutletRow } from '$lib/settings';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'taxSettingsManage', propertyId));
	const outlets = createQuery(() => ({
		queryKey: outletsKey(propertyId),
		queryFn: ({ signal }) => fetchOutlets(propertyId, signal)
	}));

	const emptyDraft = () => ({ code: '', name: '', serviceCharge: true });
	let draft = $state(emptyDraft());
	let editing = $state<{ id: string; name: string; serviceCharge: boolean } | null>(null);
	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();

	/** The outlets appear on the Taxes page and in the folio forms too. */
	async function refresh() {
		await Promise.all([
			client.invalidateQueries({ queryKey: outletsKey(propertyId) }),
			client.invalidateQueries({ queryKey: taxSettingsKey(propertyId) }),
			client.invalidateQueries({ queryKey: folioContextKey(propertyId) })
		]);
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		const body = {
			code: draft.code.trim().toUpperCase(),
			name: draft.name.trim(),
			service_charge: draft.serviceCharge
		};
		try {
			await pending.run('add', async () => {
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/outlets', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': addForm.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					addForm.failed(err);
					throw err;
				}
			});
			draft = emptyDraft();
			addForm.reset();
		} catch (err) {
			error = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	/** PATCHes the outlet; a stale `If-Match` (412) reloads the list rather than overwrite someone else's change. */
	async function update(outlet: OutletRow, body: Record<string, unknown>) {
		error = '';
		try {
			await pending.run(outlet.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/outlets/{outlet}', {
						params: {
							path: { property: propertyId, outlet: outlet.id },
							header: ifMatch(outlet.version)
						},
						body
					})
				)
			);
			if (editing?.id === outlet.id) editing = null;
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				editing = null;
				error =
					'Someone else changed this outlet. The list now shows the latest version; make your change again.';
			} else {
				error = errorMessage(err);
			}
		} finally {
			await refresh();
		}
	}
</script>

<h1>Outlets</h1>
<p class="hint">Restaurants, bars and other points of sale whose bills post to folios.</p>
{#if !manage}
	<p class="hint">Only the owner can change the outlets.</p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if outlets.error && !outlets.data}
	<p class="error" role="alert">{errorMessage(outlets.error)}</p>
	<button onclick={() => outlets.refetch()}>Retry</button>
{:else if outlets.data}
	<table aria-label="Outlets">
		<thead>
			<tr>
				<th>Code</th>
				<th>Name</th>
				<th>Service charge</th>
				<th>Active</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each outlets.data as outlet (outlet.id)}
				<tr class:inactive={!outlet.active}>
					<td>{outlet.code}</td>
					{#if editing?.id === outlet.id}
						<td>
							<input
								aria-label="Name for {outlet.code}"
								required
								maxlength="100"
								bind:value={editing.name}
							/>
						</td>
						<td>
							<label class="check">
								<input type="checkbox" bind:checked={editing.serviceCharge} /> Applies
							</label>
						</td>
						<td>{outlet.active ? 'Active' : 'Inactive'}</td>
						<td class="actions">
							<button
								aria-label="Save {outlet.code}"
								disabled={pending.has(outlet.id) || !editing.name.trim()}
								onclick={() =>
									editing &&
									update(outlet, {
										name: editing.name.trim(),
										service_charge: editing.serviceCharge
									})}>Save</button
							>
							<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
						</td>
					{:else}
						<td>{outlet.name}</td>
						<td>{outlet.serviceCharge ? 'Applies' : 'Off'}</td>
						<td>{outlet.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Edit {outlet.code}"
									disabled={pending.has(outlet.id)}
									onclick={() =>
										(editing = {
											id: outlet.id,
											name: outlet.name,
											serviceCharge: outlet.serviceCharge
										})}>Edit</button
								>
								<button
									class="secondary"
									aria-label="{outlet.active ? 'Deactivate' : 'Activate'} {outlet.code}"
									disabled={pending.has(outlet.id)}
									onclick={() => update(outlet, { active: !outlet.active })}
									>{outlet.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					{/if}
				</tr>
			{:else}
				<tr><td colspan={manage ? 5 : 4}>No outlets yet.</td></tr>
			{/each}
		</tbody>
	</table>

	{#if manage}
		<h2>Add an outlet</h2>
		<form class="inline-form" aria-label="New outlet" onsubmit={add}>
			<label>
				Code
				<input
					required
					maxlength="10"
					pattern={'[A-Za-z0-9]{1,10}'}
					placeholder="BAR"
					bind:value={draft.code}
				/>
			</label>
			<label>Name <input required maxlength="100" bind:value={draft.name} /></label>
			<label class="check">
				<input type="checkbox" bind:checked={draft.serviceCharge} /> Service charge applies
			</label>
			<button disabled={pending.has('add')}>Add outlet</button>
		</form>
		<p class="hint">The code names the outlet's bills (pos:CODE) and can't be changed later.</p>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	td input:not([type='checkbox']) {
		width: 12rem;
	}
</style>
