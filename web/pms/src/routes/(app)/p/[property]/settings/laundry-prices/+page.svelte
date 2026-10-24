<!--
	The guest laundry price list: what one item costs for one service, net of tax, in the property's base currency.
	An item code and service never change once made; the name and price can, for orders taken from then on, and an
	inactive price is left off new orders. Changed with `LaundryPricesManage`.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { formatAmount, money, parseAmount } from '$lib/folio';
	import {
		fetchLaundryPrices,
		laundryKey,
		priceGroups,
		SERVICE_OPTIONS,
		serviceLabel,
		type LaundryPrice,
		type ServiceValue
	} from '$lib/laundry';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'laundryPricesManage', propertyId));
	const prices = createQuery(() => ({
		queryKey: laundryKey(propertyId, 'prices', 'all'),
		queryFn: ({ signal }) => fetchLaundryPrices(propertyId, true, signal)
	}));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const baseCurrency = $derived(
		properties.data?.find((property) => property.id === propertyId)?.baseCurrency ?? ''
	);
	const rows = $derived(priceGroups(prices.data ?? []).flatMap((group) => group.prices));

	const emptyDraft = () => ({ itemCode: '', service: 'wash' as ServiceValue, name: '', price: '' });
	let draft = $state(emptyDraft());
	/** The row being changed, with the version it had when Edit was pressed (its `If-Match`). */
	let editing = $state<{ id: string; version: number; name: string; price: string } | null>(null);
	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();

	/** Minor units of the base currency above 0, or `null`. */
	function amount(text: string): number | null {
		if (!baseCurrency) return null;
		const minor = parseAmount(text, baseCurrency);
		return minor && minor > 0 ? minor : null;
	}

	async function refresh() {
		await client.invalidateQueries({ queryKey: laundryKey(propertyId) });
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		const price = amount(draft.price);
		if (price === null) {
			error = `Enter a price above 0 in ${baseCurrency}.`;
			return;
		}
		const body = {
			item_code: draft.itemCode.trim().toUpperCase(),
			service: draft.service,
			name: draft.name.trim(),
			price
		};
		try {
			await pending.run('add', () =>
				addForm.submit(body, async (key) =>
					unwrap(
						await rest.POST('/api/v1/properties/{property}/laundry-prices', {
							params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
							body
						})
					)
				)
			);
			draft = emptyDraft();
		} catch (err) {
			error = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	/** PATCHes the price at `version`; a stale one (412) reloads the list rather than overwrite someone else's change. */
	async function update(
		row: Pick<LaundryPrice, 'id'>,
		version: number,
		body: { name?: string; price?: number; active?: boolean }
	) {
		error = '';
		try {
			await pending.run(row.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/laundry-prices/{price}', {
						params: { path: { property: propertyId, price: row.id }, header: ifMatch(version) },
						body
					})
				)
			);
			if (editing?.id === row.id) editing = null;
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				editing = null;
				error =
					'Someone else changed this price. The list now shows the latest version; make your change again.';
			} else {
				error = errorMessage(err);
			}
		} finally {
			await refresh();
		}
	}

	function save(row: LaundryPrice) {
		if (!editing) return;
		const price = amount(editing.price);
		if (price === null) {
			error = `Enter a price above 0 in ${baseCurrency}.`;
			return;
		}
		void update(row, editing.version, { name: editing.name.trim(), price });
	}
</script>

<h1>Laundry prices</h1>
<p class="hint">
	What guests pay per item for each service, before tax, in {baseCurrency || 'the base currency'}.
</p>
{#if me.data && !manage}
	<p class="hint">Only the owner or a manager can change the laundry prices.</p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if prices.error && !prices.data}
	<p class="error" role="alert">{errorMessage(prices.error)}</p>
	<button onclick={() => prices.refetch()}>Retry</button>
{:else if prices.data}
	<table aria-label="Laundry prices">
		<thead>
			<tr>
				<th>Item</th>
				<th>Service</th>
				<th>Name</th>
				<th class="number">Price</th>
				<th>Active</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each rows as row (row.id)}
				{@const label = `${row.itemCode} ${serviceLabel(row.service)}`}
				<tr class:inactive={!row.active}>
					<td>{row.itemCode}</td>
					<td>{serviceLabel(row.service)}</td>
					{#if editing?.id === row.id}
						<td>
							<input
								aria-label="Name for {label}"
								required
								maxlength="100"
								bind:value={editing.name}
							/>
						</td>
						<td class="number">
							<input
								aria-label="Price for {label}"
								required
								inputmode="decimal"
								bind:value={editing.price}
							/>
						</td>
						<td>{row.active ? 'Active' : 'Inactive'}</td>
						<td class="actions">
							<button
								aria-label="Save {label}"
								disabled={pending.has(row.id) || !editing.name.trim()}
								onclick={() => save(row)}>Save</button
							>
							<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
						</td>
					{:else}
						<td>{row.name}</td>
						<td class="number">{baseCurrency ? money(row.price, baseCurrency) : '…'}</td>
						<td>{row.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Edit {label}"
									disabled={pending.has(row.id) || !baseCurrency}
									onclick={() =>
										(editing = {
											id: row.id,
											version: row.version,
											name: row.name,
											price: formatAmount(row.price, baseCurrency).replaceAll(',', '')
										})}>Edit</button
								>
								<button
									class="secondary"
									aria-label="{row.active ? 'Deactivate' : 'Activate'} {label}"
									disabled={pending.has(row.id)}
									onclick={() => update(row, row.version, { active: !row.active })}
									>{row.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					{/if}
				</tr>
			{:else}
				<tr><td colspan={manage ? 6 : 5}>No laundry prices yet.</td></tr>
			{/each}
		</tbody>
	</table>

	{#if manage}
		<h2>Add a price</h2>
		<form class="inline-form" aria-label="New laundry price" onsubmit={add}>
			<label>
				Item code
				<input
					required
					maxlength="20"
					pattern={'[A-Za-z0-9_]{1,20}'}
					placeholder="SHIRT"
					bind:value={draft.itemCode}
				/>
			</label>
			<label>
				Service
				<select bind:value={draft.service}>
					{#each SERVICE_OPTIONS as option (option.value)}
						<option value={option.value}>{option.label}</option>
					{/each}
				</select>
			</label>
			<label>Name <input required maxlength="100" bind:value={draft.name} /></label>
			<label>
				Price ({baseCurrency || '…'})
				<input required inputmode="decimal" bind:value={draft.price} />
			</label>
			<button disabled={pending.has('add') || !baseCurrency}>Add price</button>
		</form>
		<p class="hint">The item code and service can't be changed later; each pair has one price.</p>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.number {
		text-align: right;
	}
	td input {
		width: 10rem;
	}
</style>
