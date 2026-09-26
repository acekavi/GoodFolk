<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { SvelteMap } from 'svelte/reactivity';
	import type { BulkPreviewQuery, QuoteQuery } from '$lib/api/gql/graphql';
	import { query } from '$lib/api/graphql';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import DateGrid from '$lib/components/DateGrid.svelte';
	import { addDays, monthDays, monthOf, shiftMonth } from '$lib/inventory';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import {
		BulkPreviewDocument,
		QuoteDocument,
		batcher,
		fetchRateMonth,
		fetchRatePlans,
		formatMoney,
		formula,
		indexRates,
		parseMoney,
		ratePlansKey,
		rateRows,
		ratesKey,
		restrictionSummary,
		type RateRow
	} from '$lib/rates';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
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
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));

	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	let chosenPlan = $state('');
	const plan = $derived(
		plans.data?.ratePlans.find((p) => p.id === chosenPlan) ?? plans.data?.ratePlans[0]
	);
	const parent = $derived(
		plan?.parentId ? plans.data?.ratePlans.find((p) => p.id === plan.parentId) : undefined
	);
	const manage = $derived(!!me.data && can(me.data, 'manageRates', propertyId));
	const editable = $derived(manage && !!plan && plan.kind !== 'DERIVED');
	let chosenMonth = $state<string | null>(null);
	const month = $derived(chosenMonth ?? (businessDate ? monthOf(businessDate) : ''));
	const days = $derived(month ? monthDays(month) : []);
	const grid = createQuery(() => ({
		queryKey: ratesKey(propertyId, plan?.id ?? '', month),
		queryFn: ({ signal }) => fetchRateMonth(propertyId, plan!.id, month, signal),
		enabled: !!plan && !!month
	}));
	const rates = $derived(indexRates(grid.data ?? { prices: [], restrictions: [] }));
	const rows = $derived(plan ? rateRows(plan, roomTypes.data ?? []) : []);
	const currency = $derived(plan?.currency ?? '');
	const monthLabel = $derived(
		month
			? new Date(`${month}-01T00:00:00Z`).toLocaleDateString(undefined, {
					month: 'long',
					year: 'numeric',
					timeZone: 'UTC'
				})
			: ''
	);

	let error = $state('');
	const pending = new Pending();

	/** Prices typed but not saved yet, by `planId|roomTypeId|date|occupancy`, shown until the refetch. */
	const drafts = new SvelteMap<string, number>();
	interface Edit {
		planId: string;
		room_type_id: string;
		date: string;
		occupancy: number;
		amount: number;
	}
	const cellKey = (planId: string, roomTypeId: string, date: string, occupancy: number) =>
		`${planId}|${roomTypeId}|${date}|${occupancy}`;
	const edits = batcher<Edit>(
		(edit) => cellKey(edit.planId, edit.room_type_id, edit.date, edit.occupancy),
		async (items) => {
			const planId = items[0].planId;
			const prices = items.map(({ room_type_id, date, occupancy, amount }) => ({
				room_type_id,
				date,
				occupancy,
				amount
			}));
			try {
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/rate-plans/{plan}/prices', {
						params: { path: { property: propertyId, plan: planId } },
						body: { prices }
					})
				);
			} finally {
				await refetchPlan(planId);
				for (const edit of items) {
					const key = cellKey(planId, edit.room_type_id, edit.date, edit.occupancy);
					// A newer re-edit made while this save was in flight keeps its own draft until its
					// own save finishes; only remove the draft this save actually wrote.
					if (drafts.get(key) === edit.amount) drafts.delete(key);
				}
			}
		},
		400,
		(err) => (error = errorMessage(err))
	);

	/** Refetches every loaded month of `planId` (and, through events, the plans derived from it). */
	function refetchPlan(planId: string) {
		const prefix = `rates:${propertyId}:${planId}:`;
		return client.invalidateQueries({ predicate: (q) => String(q.queryKey[0]).startsWith(prefix) });
	}

	function price(row: RateRow, date: string): number | undefined {
		if (!plan || row.occupancy === null) return undefined;
		return (
			drafts.get(cellKey(plan.id, row.roomTypeId, date, row.occupancy)) ??
			rates.price(row.roomTypeId, date, row.occupancy)
		);
	}

	function cellLabel(row: RateRow, date: string): string {
		if (row.occupancy === null) {
			const restriction = rates.restriction(row.roomTypeId, date);
			return `${row.label} ${date}: ${(restriction && restrictionSummary(restriction)) || 'none'}`;
		}
		const amount = price(row, date);
		return `${row.label} ${date}: ${amount === undefined ? 'no price' : formatMoney(amount, currency)}`;
	}

	let editing = $state<{ row: RateRow; date: string; text: string } | null>(null);

	function startEditing(row: RateRow, date: string) {
		finishEditing();
		if (!editable || row.occupancy === null || date < businessDate) return;
		const amount = price(row, date);
		editing = {
			row,
			date,
			text: amount === undefined ? '' : formatMoney(amount, currency).replaceAll(',', '')
		};
	}

	/** Queues the edited price for saving; an empty or unchanged cell is left as it is. */
	function finishEditing() {
		const current = editing;
		editing = null;
		if (!current || !plan || current.row.occupancy === null || current.text.trim() === '') return;
		const amount = parseMoney(current.text, currency);
		if (amount === null) {
			error = `“${current.text}” is not a price.`;
			return;
		}
		if (amount === price(current.row, current.date)) return;
		error = '';
		const edit = {
			planId: plan.id,
			room_type_id: current.row.roomTypeId,
			date: current.date,
			occupancy: current.row.occupancy,
			amount
		};
		drafts.set(cellKey(edit.planId, edit.room_type_id, edit.date, edit.occupancy), amount);
		edits.add(edit);
	}

	function focusOnMount(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	function choosePlan(id: string) {
		finishEditing();
		void edits.flush();
		chosenPlan = id;
	}

	/** Range forms: `through` is the last day changed; the API takes `[from, to)`. */
	interface RangeForm {
		from: string;
		through: string;
		weekdays: number[];
		roomTypeIds: string[];
	}

	function rangeForm(): RangeForm {
		return {
			from: businessDate,
			through: businessDate,
			weekdays: [1, 2, 3, 4, 5, 6, 7],
			roomTypeIds: plan ? [...plan.roomTypeIds] : []
		};
	}

	function toggle<T>(list: T[], item: T, on: boolean): T[] {
		return on ? [...list.filter((x) => x !== item), item] : list.filter((x) => x !== item);
	}

	let bulkDialog = $state<HTMLDialogElement>();
	let bulk = $state({
		...rangeForm(),
		mode: 'percent' as 'percent' | 'amount' | 'set',
		value: ''
	});
	let preview = $state<BulkPreviewQuery['bulkChangePreview'] | null>(null);
	let dialogError = $state('');
	const bulkKeys = formKeys();

	function openBulk() {
		bulk = { ...rangeForm(), mode: 'percent', value: '' };
		preview = null;
		dialogError = '';
		bulkDialog?.showModal();
	}

	/** The bulk change's value in the API's units: basis points, or minor units (signed for `amount`). */
	function bulkValue(): number {
		const text = bulk.value.trim();
		const negative = text.startsWith('-');
		const size = text.replace(/^[-+]/, '');
		const value =
			bulk.mode === 'percent' ? Math.round(Number(size) * 100) : parseMoney(size, currency);
		if (!size || value === null || !Number.isFinite(value) || (bulk.mode === 'set' && negative)) {
			throw new Error('Enter the value as a number.');
		}
		return negative ? -value : value;
	}

	function bulkBody() {
		return {
			from: bulk.from,
			to: addDays(bulk.through, 1),
			weekdays: bulk.weekdays.toSorted(),
			room_type_ids: bulk.roomTypeIds,
			change: { mode: bulk.mode, value: bulkValue() }
		};
	}

	async function previewBulk() {
		if (!plan) return;
		dialogError = '';
		try {
			const body = bulkBody();
			preview = (
				await query(BulkPreviewDocument, {
					propertyId,
					ratePlanId: plan.id,
					from: body.from,
					to: body.to,
					weekdays: body.weekdays,
					roomTypeIds: body.room_type_ids,
					mode: body.change.mode.toUpperCase() as 'PERCENT' | 'AMOUNT' | 'SET',
					value: body.change.value
				})
			).bulkChangePreview;
		} catch (err) {
			preview = null;
			dialogError = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
		}
	}

	async function applyBulk(event: SubmitEvent) {
		event.preventDefault();
		if (!plan) return;
		const planId = plan.id;
		dialogError = '';
		try {
			const body = bulkBody();
			await pending.run('bulk', async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/rate-plans/{plan}/bulk-change', {
						params: {
							path: { property: propertyId, plan: planId },
							header: { 'Idempotency-Key': bulkKeys.keyFor(body) }
						},
						body
					})
				)
			);
			bulkKeys.reset();
			bulkDialog?.close();
		} catch (err) {
			bulkKeys.failed(err);
			dialogError = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
		} finally {
			await refetchPlan(planId);
		}
	}

	let restrictionsDialog = $state<HTMLDialogElement>();
	let restriction = $state({
		...rangeForm(),
		closed: '',
		minStay: null as number | null,
		maxStay: null as number | null,
		arrival: '',
		departure: ''
	});

	function openRestrictions() {
		restriction = {
			...rangeForm(),
			closed: '',
			minStay: null,
			maxStay: null,
			arrival: '',
			departure: ''
		};
		dialogError = '';
		restrictionsDialog?.showModal();
	}

	/** `''` leaves a yes/no restriction as it is. */
	const flag = (value: string) => (value === '' ? undefined : value === 'true');
	/** A blank number field is `null`: leave that stay limit as it is. */
	const nights = (value: number | null) => value ?? undefined;

	async function saveRestrictions(event: SubmitEvent) {
		event.preventDefault();
		if (!plan) return;
		const planId = plan.id;
		dialogError = '';
		try {
			await pending.run('restrictions', async () =>
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/rate-plans/{plan}/restrictions', {
						params: { path: { property: propertyId, plan: planId } },
						body: {
							from: restriction.from,
							to: addDays(restriction.through, 1),
							weekdays: restriction.weekdays.toSorted(),
							room_type_ids: restriction.roomTypeIds,
							closed: flag(restriction.closed),
							min_stay: nights(restriction.minStay),
							max_stay: nights(restriction.maxStay),
							closed_to_arrival: flag(restriction.arrival),
							closed_to_departure: flag(restriction.departure)
						}
					})
				)
			);
			restrictionsDialog?.close();
		} catch (err) {
			dialogError = errorMessage(err);
		} finally {
			await refetchPlan(planId);
		}
	}

	let stay = $state({
		roomTypeId: '',
		mealPlan: 'RO' as 'RO' | 'BB' | 'HB' | 'FB',
		checkIn: '',
		checkOut: '',
		adults: 2,
		children: 0,
		residency: 'NON_RESIDENT' as 'RESIDENT' | 'NON_RESIDENT'
	});
	let quoted = $state<QuoteQuery['quote'] | null>(null);
	let quoteError = $state('');

	async function runQuote(event: SubmitEvent) {
		event.preventDefault();
		if (!plan) return;
		quoteError = '';
		try {
			quoted = (
				await query(QuoteDocument, {
					propertyId,
					ratePlanId: plan.id,
					roomTypeId: stay.roomTypeId || plan.roomTypeIds[0],
					mealPlan: plan.allowedMealPlans.includes(stay.mealPlan)
						? stay.mealPlan
						: plan.allowedMealPlans[0],
					checkIn: stay.checkIn,
					checkOut: stay.checkOut,
					adults: stay.adults,
					children: stay.children,
					residency: stay.residency
				})
			).quote;
		} catch (err) {
			quoted = null;
			quoteError = errorMessage(err);
		}
	}

	const typeCode = (id: string) => roomTypes.data?.find((type) => type.id === id)?.code ?? '?';
</script>

<h1>Rates</h1>
{#if plans.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(plans.error ?? roomTypes.error)}</p>
{:else if plans.data && roomTypes.data}
	{#if !plan}
		<p>No rate plans yet. Add one on the Rate plans page.</p>
	{:else}
		<div class="inline-form">
			<label>
				Rate plan
				<select value={plan.id} onchange={(event) => choosePlan(event.currentTarget.value)}>
					{#each plans.data.ratePlans as option (option.id)}
						<option value={option.id}>{option.code}</option>
					{/each}
				</select>
			</label>
			<button
				class="secondary"
				aria-label="Previous month"
				onclick={() => (chosenMonth = shiftMonth(month, -1))}>←</button
			>
			<h2 class="month">{monthLabel}</h2>
			<button
				class="secondary"
				aria-label="Next month"
				onclick={() => (chosenMonth = shiftMonth(month, 1))}>→</button
			>
			<span class="hint"
				>Business date: <span data-testid="business-date">{businessDate}</span></span
			>
			{#if editable}
				<button onclick={openBulk}>Bulk change…</button>
			{/if}
			{#if manage && !(plan.kind === 'DERIVED' && plan.inheritRestrictions)}
				<button class="secondary" onclick={openRestrictions}>Restrictions…</button>
			{/if}
		</div>
		<p class="hint">
			{plan.name} · {plan.currency}
			{#if parent}
				· Derived from {parent.code}: {formula(plan, parent)}. Its prices follow {parent.code} and cannot
				be edited here.
			{:else if editable}
				· Click a price, or use the arrow keys and Enter, to change it; changes save as you go.
			{/if}
		</p>
		{#if error}<p class="error" role="alert">{error}</p>{/if}

		{#if grid.error}
			<p class="error" role="alert">{errorMessage(grid.error)}</p>
		{:else if grid.data}
			{#if rows.length === 0}
				<p>This plan sells no active room types.</p>
			{:else}
				<DateGrid
					label="Prices"
					{rows}
					columns={days}
					{cellLabel}
					railWidth={200}
					columnWidth={76}
					initialColumn={Math.max(0, days.indexOf(businessDate))}
					onactivate={startEditing}
				>
					{#snippet header(date)}
						<span class="day" class:today={date === businessDate}>
							<small>{WEEKDAYS[(new Date(`${date}T00:00:00Z`).getUTCDay() + 6) % 7]}</small>
							{Number(date.slice(8))}
						</span>
					{/snippet}
					{#snippet cell(row, date)}
						{#if editing && editing.row.id === row.id && editing.date === date}
							<input
								class="price"
								aria-label="Price for {row.label} on {date}"
								inputmode="decimal"
								bind:value={editing.text}
								use:focusOnMount
								onkeydown={(event) => {
									event.stopPropagation();
									if (event.key === 'Enter') finishEditing();
									if (event.key === 'Escape') editing = null;
								}}
								onblur={finishEditing}
							/>
						{:else if row.occupancy === null}
							{@const r = rates.restriction(row.roomTypeId, date)}
							<small class="restriction">{r ? restrictionSummary(r) : ''}</small>
						{:else}
							{@const amount = price(row, date)}
							<span
								class:draft={plan &&
									drafts.has(cellKey(plan.id, row.roomTypeId, date, row.occupancy))}
								>{amount === undefined ? '–' : formatMoney(amount, currency)}</span
							>
						{/if}
					{/snippet}
				</DateGrid>
			{/if}
		{:else}
			<p>Loading…</p>
		{/if}

		<form class="inline-form quote" aria-label="Quote" onsubmit={runQuote}>
			<h2>Quote a stay on {plan.code}</h2>
			<label>
				Room type
				<select bind:value={stay.roomTypeId}>
					{#each plan.roomTypeIds as id (id)}<option value={id}>{typeCode(id)}</option>{/each}
				</select>
			</label>
			<label>
				Meal plan
				<select bind:value={stay.mealPlan}>
					{#each plan.allowedMealPlans as mealPlan (mealPlan)}<option value={mealPlan}
							>{mealPlan}</option
						>{/each}
				</select>
			</label>
			<label>Check-in <input type="date" required bind:value={stay.checkIn} /></label>
			<label>Check-out <input type="date" required bind:value={stay.checkOut} /></label>
			<label>Adults <input type="number" min="1" max="50" bind:value={stay.adults} /></label>
			<label>Children <input type="number" min="0" max="50" bind:value={stay.children} /></label>
			<label>
				Residency
				<select bind:value={stay.residency}>
					<option value="NON_RESIDENT">Non-resident</option>
					<option value="RESIDENT">Resident</option>
				</select>
			</label>
			<button>Quote</button>
		</form>
		{#if quoteError}<p class="error" role="alert">{quoteError}</p>{/if}
		{#if quoted}
			<section aria-label="Quote result" aria-live="polite">
				<table>
					<thead><tr><th>Night</th><th>Room</th><th>Meals</th></tr></thead>
					<tbody>
						{#each quoted.nights as night (night.date)}
							<tr>
								<td>{night.date}</td>
								<td>{formatMoney(night.room, quoted.currency)}</td>
								<td>{formatMoney(night.meal, quoted.currency)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<p>
					<strong>Total {formatMoney(quoted.total, quoted.currency)} {quoted.currency}</strong>
					{quoted.restrictionsOk ? '· can be sold' : '· cannot be sold as quoted:'}
				</p>
				<ul>
					{#each quoted.violations as violation, index (index)}<li class="error">
							{violation.message}
						</li>{/each}
				</ul>
			</section>
		{/if}
	{/if}
{:else}
	<p>Loading…</p>
{/if}

{#snippet rangeFields(form: RangeForm)}
	<label>From <input type="date" required bind:value={form.from} /></label>
	<label>Through <input type="date" required bind:value={form.through} /></label>
	<fieldset>
		<legend>Days of the week</legend>
		{#each WEEKDAYS as day, index (day)}
			<label class="check">
				<input
					type="checkbox"
					checked={form.weekdays.includes(index + 1)}
					onchange={(event) =>
						(form.weekdays = toggle(form.weekdays, index + 1, event.currentTarget.checked))}
				/>
				{day}
			</label>
		{/each}
	</fieldset>
	<fieldset>
		<legend>Room types</legend>
		{#each plan?.roomTypeIds ?? [] as id (id)}
			<label class="check">
				<input
					type="checkbox"
					checked={form.roomTypeIds.includes(id)}
					onchange={(event) =>
						(form.roomTypeIds = toggle(form.roomTypeIds, id, event.currentTarget.checked))}
				/>
				{typeCode(id)}
			</label>
		{/each}
	</fieldset>
{/snippet}

<dialog bind:this={bulkDialog} aria-labelledby="bulk-title">
	<form class="form" onsubmit={applyBulk}>
		<h2 id="bulk-title">Bulk change</h2>
		{@render rangeFields(bulk)}
		<label>
			Change
			<select bind:value={bulk.mode} onchange={() => (preview = null)}>
				<option value="percent">By a percentage (e.g. 10 or -5)</option>
				<option value="amount">By an amount (e.g. 5.00 or -5.00)</option>
				<option value="set">Set to</option>
			</select>
		</label>
		<label
			>Value <input
				required
				inputmode="decimal"
				bind:value={bulk.value}
				oninput={() => (preview = null)}
			/></label
		>
		{#if preview}
			<p data-testid="preview-total">
				{preview.total}
				{preview.total === 1 ? 'price changes' : 'prices change'}{preview.total >
				preview.cells.length
					? `; the first ${preview.cells.length}:`
					: ':'}
			</p>
			<table aria-label="Changes">
				<tbody>
					{#each preview.cells as change (`${change.date}|${change.roomTypeId}|${change.occupancy}`)}
						<tr>
							<td>{change.date}</td>
							<td>{typeCode(change.roomTypeId)} · {change.occupancy}</td>
							<td
								>{change.before === null || change.before === undefined
									? '–'
									: formatMoney(change.before, currency)} → {formatMoney(
									change.after,
									currency
								)}</td
							>
						</tr>
					{/each}
				</tbody>
			</table>
			{#if parent === undefined && plans.data?.ratePlans.some((p) => p.parentId === plan?.id)}
				<p class="hint">Plans derived from {plan?.code} change with it.</p>
			{/if}
		{/if}
		{#if dialogError}<p class="error" role="alert">{dialogError}</p>{/if}
		<div class="actions">
			<button type="button" class="secondary" onclick={previewBulk}>Preview</button>
			<button disabled={!preview || preview.total === 0 || pending.has('bulk')}>Apply</button>
			<button type="button" class="secondary" onclick={() => bulkDialog?.close()}>Cancel</button>
		</div>
	</form>
</dialog>

<dialog bind:this={restrictionsDialog} aria-labelledby="restrictions-title">
	<form class="form" onsubmit={saveRestrictions}>
		<h2 id="restrictions-title">Restrictions</h2>
		{@render rangeFields(restriction)}
		<p class="hint">Fields left blank stay as they are.</p>
		{#each [['closed', 'Closed'], ['arrival', 'Closed to arrival'], ['departure', 'Closed to departure']] as const as [field, label] (field)}
			<label>
				{label}
				<select bind:value={restriction[field]}>
					<option value="">Leave as it is</option>
					<option value="true">Yes</option>
					<option value="false">No</option>
				</select>
			</label>
		{/each}
		<label
			>Minimum stay <input
				type="number"
				min="1"
				max="365"
				bind:value={restriction.minStay}
			/></label
		>
		<label
			>Maximum stay <input
				type="number"
				min="1"
				max="365"
				bind:value={restriction.maxStay}
			/></label
		>
		{#if dialogError}<p class="error" role="alert">{dialogError}</p>{/if}
		<div class="actions">
			<button disabled={pending.has('restrictions')}>Save restrictions</button>
			<button type="button" class="secondary" onclick={() => restrictionsDialog?.close()}
				>Cancel</button
			>
		</div>
	</form>
</dialog>

<style>
	.month {
		margin: 0;
		min-width: 12rem;
		text-align: center;
	}
	.day {
		display: grid;
		text-align: center;
		line-height: 1.1;
	}
	.today {
		color: var(--accent);
		font-weight: 600;
	}
	.price {
		width: 100%;
		padding: 0.2rem;
		text-align: right;
	}
	.draft {
		font-style: italic;
		color: var(--muted);
	}
	.restriction {
		font-size: 0.75em;
		color: var(--danger);
		text-align: center;
	}
	.quote {
		margin-top: calc(var(--space) * 2);
	}
	.quote h2 {
		flex-basis: 100%;
		margin: 0;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}
</style>
