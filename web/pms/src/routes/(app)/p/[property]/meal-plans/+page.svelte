<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import {
		fetchRatePlans,
		formatMoney,
		parseMoney,
		ratePlansKey,
		type MealSupplement
	} from '$lib/rates';
	import { can, fetchMe } from '$lib/session';

	const MEAL_PLANS = [
		{ value: 'BB', label: 'BB · bed and breakfast' },
		{ value: 'HB', label: 'HB · half board' },
		{ value: 'FB', label: 'FB · full board' }
	] as const;

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const plans = createQuery(() => ({
		queryKey: ratePlansKey(propertyId),
		queryFn: ({ signal }) => fetchRatePlans(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRates', propertyId));
	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	/** Supplements by currency; the list is already ordered by currency, meal plan and start. */
	const byCurrency = $derived(
		Map.groupBy(plans.data?.mealSupplements ?? [], (supplement) => supplement.currency)
	);

	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();
	let draft = $state({
		mealPlan: 'BB' as 'BB' | 'HB' | 'FB',
		currency: '',
		adult: '',
		child: '0',
		from: '',
		to: ''
	});
	let editing = $state<{ id: string; adult: string; child: string; to: string } | null>(null);

	function amount(text: string, currency: string): number {
		const value = parseMoney(text, currency);
		if (value === null) throw new Error(`“${text}” is not an amount.`);
		return value;
	}

	async function run(key: string, command: () => Promise<unknown>) {
		error = '';
		try {
			await pending.run(key, command);
			return true;
		} catch (err) {
			error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
			return false;
		} finally {
			await client.invalidateQueries({ queryKey: ratePlansKey(propertyId) });
		}
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		const currency = draft.currency.toUpperCase();
		let body;
		try {
			body = {
				meal_plan: draft.mealPlan,
				currency,
				adult_amount: amount(draft.adult, currency),
				child_amount: amount(draft.child, currency),
				from: draft.from || businessDate,
				to: draft.to || null
			};
		} catch (err) {
			error = (err as Error).message;
			return;
		}
		const added = await run('add', async () => {
			try {
				unwrap(
					await rest.POST('/api/v1/properties/{property}/meal-supplements', {
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
		if (added) addForm.reset();
	}

	function edit(supplement: MealSupplement) {
		editing = {
			id: supplement.id,
			adult: formatMoney(supplement.adultAmount, supplement.currency).replaceAll(',', ''),
			child: formatMoney(supplement.childAmount, supplement.currency).replaceAll(',', ''),
			to: supplement.to ?? ''
		};
	}

	async function save(supplement: MealSupplement) {
		if (!editing) return;
		const changes = editing;
		const saved = await run(supplement.id, async () =>
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/meal-supplements/{supplement}', {
					params: {
						path: { property: propertyId, supplement: supplement.id },
						header: ifMatch(supplement.version)
					},
					body: {
						adult_amount: amount(changes.adult, supplement.currency),
						child_amount: amount(changes.child, supplement.currency),
						to: changes.to || null
					}
				})
			)
		);
		if (saved) editing = null;
	}
</script>

<h1>Meal plans</h1>
<p class="hint">
	Room only (RO) is free. Bed and breakfast, half board and full board are charged per person per
	night on top of the room price, in the rate plan's currency. Business date:
	<span data-testid="business-date">{businessDate}</span>
</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if plans.error}
	<p class="error" role="alert">{errorMessage(plans.error)}</p>
{:else if plans.data}
	{#each byCurrency as [currency, supplements] (currency)}
		<h2>{currency}</h2>
		<table aria-label="Meal supplements in {currency}">
			<thead>
				<tr>
					<th>Meal plan</th>
					<th>Per adult</th>
					<th>Per child</th>
					<th>From</th>
					<th>Until (first night not charged)</th>
					{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
				</tr>
			</thead>
			<tbody>
				{#each supplements as supplement (supplement.id)}
					{@const code = supplement.mealPlan}
					<tr>
						<td>{code}</td>
						{#if editing?.id === supplement.id}
							<td><input aria-label="Per adult for {code}" bind:value={editing.adult} /></td>
							<td><input aria-label="Per child for {code}" bind:value={editing.child} /></td>
							<td>{supplement.from}</td>
							<td><input type="date" aria-label="Until for {code}" bind:value={editing.to} /></td>
							<td class="actions">
								<button
									aria-label="Save {code}"
									disabled={pending.has(supplement.id)}
									onclick={() => save(supplement)}>Save</button
								>
								<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
							</td>
						{:else}
							<td>{formatMoney(supplement.adultAmount, currency)}</td>
							<td>{formatMoney(supplement.childAmount, currency)}</td>
							<td>{supplement.from}</td>
							<td>{supplement.to ?? 'Until further notice'}</td>
							{#if manage}
								<td>
									<button
										class="secondary"
										aria-label="Edit {code}"
										onclick={() => edit(supplement)}>Edit</button
									>
								</td>
							{/if}
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p>No meal supplements yet.</p>
	{/each}

	{#if manage}
		<h2>Add a supplement</h2>
		<form class="inline-form" aria-label="New meal supplement" onsubmit={add}>
			<label>
				Meal plan
				<select bind:value={draft.mealPlan}>
					{#each MEAL_PLANS as mealPlan (mealPlan.value)}
						<option value={mealPlan.value}>{mealPlan.label}</option>
					{/each}
				</select>
			</label>
			<label>Currency <input required pattern={'[A-Za-z]{3}'} bind:value={draft.currency} /></label>
			<label>Per adult <input required inputmode="decimal" bind:value={draft.adult} /></label>
			<label>Per child <input required inputmode="decimal" bind:value={draft.child} /></label>
			<label>From <input type="date" bind:value={draft.from} /></label>
			<label>Until (optional) <input type="date" bind:value={draft.to} /></label>
			<button disabled={pending.has('add')}>Add supplement</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}
