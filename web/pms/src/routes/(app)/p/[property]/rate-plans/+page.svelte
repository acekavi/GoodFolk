<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import type { components } from '$lib/api/openapi';
	import { Pending } from '$lib/pending.svelte';
	import {
		fetchRatePlans,
		formatMoney,
		formula,
		parseMoney,
		ratePlansKey,
		type RatePlan
	} from '$lib/rates';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	type Schemas = components['schemas'];
	type Kind = Schemas['PlanKind'];
	type Segment = Schemas['Segment'];
	type MealPlan = Schemas['MealPlan'];

	const SEGMENTS: { value: Segment; label: string }[] = [
		{ value: 'FIT_F', label: 'FIT-F (foreign independent travellers)' },
		{ value: 'FIT_L', label: 'FIT-L (local independent travellers)' },
		{ value: 'OTA', label: 'OTA (online travel agents)' },
		{ value: 'TA', label: 'TA (travel agent contracts)' },
		{ value: 'IBE', label: 'IBE (own booking engine)' }
	];
	const MEAL_PLANS: MealPlan[] = ['RO', 'BB', 'HB', 'FB'];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const plans = createQuery(() => ({
		queryKey: ratePlansKey(propertyId),
		queryFn: ({ signal }) => fetchRatePlans(propertyId, signal)
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRates', propertyId));
	const byId = $derived(new Map((plans.data?.ratePlans ?? []).map((plan) => [plan.id, plan])));

	interface Draft {
		id: string | null;
		version: number;
		code: string;
		name: string;
		kind: Kind;
		segment: Segment;
		residency: '' | 'resident' | 'non_resident';
		currency: string;
		parentId: string;
		deriveMode: 'percent' | 'amount';
		/** Percent (`15`, `-5`) or a signed amount (`-10.50`). */
		deriveValue: string;
		roundingStep: string;
		extraAdult: string;
		inherit: boolean;
		mealPlans: MealPlan[];
		roomTypeIds: string[];
		policyId: string;
		active: boolean;
	}

	let draft = $state<Draft | null>(null);
	let error = $state('');
	const pending = new Pending();
	const createForm = formKeys();
	const parent = $derived(draft?.kind === 'derived' ? byId.get(draft.parentId) : undefined);
	const currency = $derived(parent?.currency ?? draft?.currency ?? '');

	function newDraft(): Draft {
		return {
			id: null,
			version: 0,
			code: '',
			name: '',
			kind: 'standard',
			segment: 'IBE',
			residency: '',
			currency: '',
			parentId: '',
			deriveMode: 'percent',
			deriveValue: '',
			roundingStep: '1',
			extraAdult: '0',
			inherit: false,
			mealPlans: ['RO'],
			roomTypeIds: (roomTypes.data ?? []).filter((t) => t.active).map((t) => t.id),
			policyId: '',
			active: true
		};
	}

	function editDraft(plan: RatePlan): Draft {
		const value = plan.deriveValue ?? 0;
		const sign = value < 0 ? '-' : '';
		return {
			id: plan.id,
			version: plan.version,
			code: plan.code,
			name: plan.name,
			kind: plan.kind.toLowerCase() as Kind,
			segment: plan.segment,
			residency: (plan.residency?.toLowerCase() ?? '') as Draft['residency'],
			currency: plan.currency,
			parentId: plan.parentId ?? '',
			deriveMode: plan.deriveMode === 'AMOUNT' ? 'amount' : 'percent',
			deriveValue:
				plan.deriveMode === 'AMOUNT'
					? sign + formatMoney(Math.abs(value), plan.currency).replaceAll(',', '')
					: String(value / 100),
			roundingStep: formatMoney(plan.roundingStep, plan.currency).replaceAll(',', ''),
			extraAdult: formatMoney(plan.extraAdultAmount, plan.currency).replaceAll(',', ''),
			inherit: plan.inheritRestrictions,
			mealPlans: [...plan.allowedMealPlans],
			roomTypeIds: [...plan.roomTypeIds],
			policyId: plan.cancellationPolicyId ?? '',
			active: plan.active
		};
	}

	/** The derivation fields of a derived plan's request, in the units the API takes. */
	function derivation(d: Draft) {
		const negative = d.deriveValue.trim().startsWith('-');
		const size = d.deriveValue.trim().replace(/^[-+]/, '');
		const value =
			d.deriveMode === 'percent'
				? Math.round(Number(size) * 100)
				: (parseMoney(size, currency) ?? Number.NaN);
		if (!size || !Number.isFinite(value)) throw new Error('Enter the change as a number.');
		return {
			parent_id: d.parentId,
			derive_mode: d.deriveMode,
			derive_value: negative ? -value : value,
			inherit_restrictions: d.inherit
		};
	}

	function money(text: string, label: string): number {
		const amount = parseMoney(text, currency);
		if (amount === null) throw new Error(`Enter ${label} as an amount, e.g. 1.00.`);
		return amount;
	}

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!draft) return;
		const d = draft;
		error = '';
		try {
			const common = {
				name: d.name,
				segment: d.segment,
				rounding_step: money(d.roundingStep, 'the rounding step'),
				extra_adult_amount: money(d.extraAdult, 'the extra adult amount'),
				allowed_meal_plans: d.mealPlans,
				room_type_ids: d.roomTypeIds,
				...(d.kind === 'derived' ? derivation(d) : {})
			};
			await pending.run('plan', async () => {
				if (d.id) {
					unwrap(
						await rest.PATCH('/api/v1/properties/{property}/rate-plans/{plan}', {
							params: { path: { property: propertyId, plan: d.id }, header: ifMatch(d.version) },
							body: {
								...common,
								residency: d.residency || null,
								cancellation_policy_id: d.policyId || null,
								active: d.active
							}
						})
					);
				} else {
					const body = {
						...common,
						code: d.code.toUpperCase(),
						kind: d.kind,
						currency: currency.toUpperCase(),
						residency: d.residency || undefined,
						cancellation_policy_id: d.policyId || undefined
					};
					unwrap(
						await rest.POST('/api/v1/properties/{property}/rate-plans', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': createForm.keyFor(body) }
							},
							body
						})
					);
					createForm.reset();
				}
			});
			draft = null;
		} catch (err) {
			error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
			if (!draft?.id) createForm.failed(err);
		} finally {
			await client.invalidateQueries({ queryKey: ratePlansKey(propertyId) });
		}
	}

	function residencyLabel(plan: RatePlan): string {
		if (plan.residency === 'RESIDENT') return 'Residents only';
		if (plan.residency === 'NON_RESIDENT') return 'Non-residents only';
		return 'Any guest';
	}

	function toggle<T>(list: T[], item: T, on: boolean): T[] {
		return on ? [...list.filter((x) => x !== item), item] : list.filter((x) => x !== item);
	}
</script>

<h1>Rate plans</h1>
{#if error && !draft}<p class="error" role="alert">{error}</p>{/if}

{#if plans.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(plans.error ?? roomTypes.error)}</p>
{:else if plans.data && roomTypes.data}
	<table aria-label="Rate plans">
		<thead>
			<tr>
				<th>Code</th>
				<th>Name</th>
				<th>Sold to</th>
				<th>Pricing</th>
				<th>Status</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each plans.data.ratePlans as plan (plan.id)}
				<tr class:inactive={!plan.active}>
					<td style:padding-left="{0.6 + plan.depth * 1.5}rem">
						{#if plan.depth > 0}<span aria-hidden="true">↳ </span>{/if}{plan.code}
					</td>
					<td>{plan.name}</td>
					<td>
						<span class="badge">{plan.segment.replace('_', '-')}</span>
						<span class="badge">{plan.currency}</span>
						<span class="badge">{residencyLabel(plan)}</span>
					</td>
					<td>
						{#if plan.parentId && byId.get(plan.parentId)}
							{formula(plan, byId.get(plan.parentId)!)}
						{:else}
							{plan.kind === 'CUSTOM' ? 'Custom, priced by hand' : 'Priced by hand'}
						{/if}
					</td>
					<td>{plan.active ? 'Active' : 'Inactive'}</td>
					{#if manage}
						<td>
							<button
								class="secondary"
								aria-label="Edit {plan.code}"
								onclick={() => {
									error = '';
									draft = editDraft(plan);
								}}>Edit</button
							>
						</td>
					{/if}
				</tr>
			{:else}
				<tr><td colspan="6">No rate plans yet.</td></tr>
			{/each}
		</tbody>
	</table>
	{#if manage && !draft}
		<button
			onclick={() => {
				error = '';
				draft = newDraft();
			}}>New rate plan</button
		>
	{/if}

	{#if draft}
		<form class="form panel" aria-label="Rate plan" onsubmit={save}>
			<h2>{draft.id ? `Edit ${draft.code}` : 'New rate plan'}</h2>
			<label
				>Code <input
					required
					pattern={'[A-Za-z0-9_\\-]{1,20}'}
					disabled={!!draft.id}
					bind:value={draft.code}
				/></label
			>
			<label>Name <input required maxlength="100" bind:value={draft.name} /></label>
			<label>
				Kind
				<select disabled={!!draft.id} bind:value={draft.kind}>
					<option value="standard">Standard (priced by hand, can have derived plans)</option>
					<option value="derived">Derived (priced from another plan)</option>
					<option value="custom">Custom (priced by hand, stands alone)</option>
				</select>
			</label>
			{#if draft.kind === 'derived'}
				<label>
					Derived from
					<select required bind:value={draft.parentId}>
						<option value="" disabled>Choose a plan</option>
						{#each plans.data.ratePlans as plan (plan.id)}
							{#if plan.kind !== 'CUSTOM' && plan.id !== draft.id}
								<option value={plan.id}>{plan.code}</option>
							{/if}
						{/each}
					</select>
				</label>
				<label>
					Change by
					<select bind:value={draft.deriveMode}>
						<option value="percent">Percentage</option>
						<option value="amount">Amount</option>
					</select>
				</label>
				<label
					>{draft.deriveMode === 'percent' ? 'Change (%)' : `Change (${currency})`}
					<input required inputmode="decimal" bind:value={draft.deriveValue} /></label
				>
				<label class="check"
					><input type="checkbox" bind:checked={draft.inherit} /> Inherit the parent's restrictions</label
				>
			{/if}
			<label>
				Currency
				<input
					required
					pattern={'[A-Za-z]{3}'}
					disabled={!!draft.id || draft.kind === 'derived'}
					value={currency}
					oninput={(event) => draft && (draft.currency = event.currentTarget.value)}
				/>
			</label>
			<label>
				Segment
				<select bind:value={draft.segment}>
					{#each SEGMENTS as segment (segment.value)}
						<option value={segment.value}>{segment.label}</option>
					{/each}
				</select>
			</label>
			<label>
				Sold to
				<select bind:value={draft.residency}>
					<option value=""
						>{draft.segment.startsWith('FIT') ? 'As the segment says' : 'Any guest'}</option
					>
					<option value="resident">Residents only</option>
					<option value="non_resident">Non-residents only</option>
				</select>
			</label>
			<label>Round prices to <input inputmode="decimal" bind:value={draft.roundingStep} /></label>
			<label
				>Each extra adult above a priced occupancy <input
					inputmode="decimal"
					bind:value={draft.extraAdult}
				/></label
			>
			<fieldset>
				<legend>Room types sold</legend>
				{#each roomTypes.data.filter((t) => t.active || draft?.roomTypeIds.includes(t.id)) as type (type.id)}
					<label class="check">
						<input
							type="checkbox"
							checked={draft.roomTypeIds.includes(type.id)}
							onchange={(event) =>
								draft &&
								(draft.roomTypeIds = toggle(
									draft.roomTypeIds,
									type.id,
									event.currentTarget.checked
								))}
						/>
						{type.code} · {type.name}
					</label>
				{/each}
			</fieldset>
			<fieldset>
				<legend>Meal plans sold</legend>
				{#each MEAL_PLANS as mealPlan (mealPlan)}
					<label class="check">
						<input
							type="checkbox"
							checked={draft.mealPlans.includes(mealPlan)}
							onchange={(event) =>
								draft &&
								(draft.mealPlans = toggle(draft.mealPlans, mealPlan, event.currentTarget.checked))}
						/>
						{mealPlan}
					</label>
				{/each}
			</fieldset>
			<label>
				Cancellation policy
				<select bind:value={draft.policyId}>
					<option value="">None</option>
					{#each plans.data.cancellationPolicies as policy (policy.id)}
						<option value={policy.id}>{policy.name}</option>
					{/each}
				</select>
			</label>
			{#if draft.id}
				<label class="check"><input type="checkbox" bind:checked={draft.active} /> Active</label>
			{/if}
			{#if error}<p class="error" role="alert">{error}</p>{/if}
			<div class="actions">
				<button disabled={pending.has('plan')}>Save plan</button>
				<button type="button" class="secondary" onclick={() => (draft = null)}>Cancel</button>
			</div>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.badge {
		display: inline-block;
		padding: 0 0.4rem;
		margin-right: 0.25rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		font-size: 0.85em;
	}
	.panel {
		margin-top: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		max-width: 32rem;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
</style>
