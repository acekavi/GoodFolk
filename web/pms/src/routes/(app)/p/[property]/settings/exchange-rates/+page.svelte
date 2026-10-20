<!--
	The property's exchange rates over the last 30 days, per accepted currency, with the import's alert, and a
	rate set by hand for `FxOverride`. A rate is in force from its date until the next one, so a day without a
	row uses the latest earlier rate. Refetched on the server's `fx:<property>` event.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { rest, unwrap } from '$lib/api/rest';
	import { addDays } from '$lib/inventory';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';
	import { exchangeRatesKey, fetchExchangeRates, fxKey, isValidRate } from '$lib/settings';

	const DAYS = 30;

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const override = $derived(!!me.data && can(me.data, 'fxOverride', propertyId));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	// The window ends on the business date, or today if that is later: the import stores today's rate.
	const today = new Date().toLocaleDateString('en-CA');
	const end = $derived(businessDate && businessDate > today ? businessDate : today);
	const from = $derived(addDays(end, 1 - DAYS));
	const to = $derived(addDays(end, 1));

	const rates = createQuery(() => ({
		queryKey: exchangeRatesKey(propertyId, from, to),
		queryFn: ({ signal }) => fetchExchangeRates(propertyId, from, to, signal),
		enabled: !!businessDate
	}));
	const foreign = $derived(
		rates.data
			? rates.data.propertyTaxSettings.acceptedCurrencies.filter(
					(currency) => currency !== rates.data.propertyTaxSettings.baseCurrency
				)
			: []
	);
	const byCurrency = $derived(
		new Map(
			foreign.map((currency) => [
				currency,
				(rates.data?.fxRates ?? [])
					.filter((rate) => rate.currency === currency)
					.sort((a, b) => b.date.localeCompare(a.date))
			])
		)
	);

	let form = $state({ date: '', currency: '', rate: '' });
	let problem = $state('');
	let saved = $state('');
	const pending = new Pending();

	// The form starts on the business date and the first foreign currency.
	$effect(() => {
		if (!form.date && businessDate) form.date = businessDate;
		if (!form.currency && foreign.length > 0) form.currency = foreign[0];
	});

	async function setRate(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		saved = '';
		const rate = form.rate.trim();
		if (!isValidRate(rate)) {
			problem = 'Enter the rate as a positive number with at most 6 decimals, such as 301.25.';
			return;
		}
		const { date, currency } = form;
		try {
			await pending.run('rate', async () =>
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/fx-rates/{date}/{currency}', {
						params: { path: { property: propertyId, date, currency } },
						body: { rate }
					})
				)
			);
			saved = `${currency} on ${date} is now ${rate}.`;
			form.rate = '';
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: fxKey(propertyId) });
		}
	}
</script>

<h1>Exchange rates</h1>
<p class="hint">
	{rates.data?.propertyTaxSettings.baseCurrency ?? 'Base currency'} per one unit of each accepted currency.
	A rate is in force from its date until the next one.
</p>

{#if rates.data?.fxAlert}
	<p class="alert" role="alert">{rates.data.fxAlert.message}</p>
{/if}

{#if rates.error && !rates.data}
	<p class="error" role="alert">{errorMessage(rates.error)}</p>
	<button onclick={() => rates.refetch()}>Retry</button>
{:else if rates.data}
	{#if override && foreign.length > 0}
		<h2>Set a rate</h2>
		<form class="inline-form" aria-label="Set a rate" onsubmit={setRate}>
			<label>Date <input type="date" required bind:value={form.date} /></label>
			<label>
				Currency
				<select required bind:value={form.currency}>
					{#each foreign as currency (currency)}
						<option value={currency}>{currency}</option>
					{/each}
				</select>
			</label>
			<label>
				Rate
				<input
					required
					inputmode="decimal"
					autocomplete="off"
					placeholder="301.25"
					bind:value={form.rate}
				/>
			</label>
			<button disabled={pending.has('rate')}>Set rate</button>
		</form>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		{#if saved}<p class="hint" role="status">{saved}</p>{/if}
	{/if}

	{#if foreign.length === 0}
		<p class="hint">The property accepts only its base currency.</p>
	{/if}
	{#each foreign as currency (currency)}
		{@const list = byCurrency.get(currency) ?? []}
		<h2>{currency}</h2>
		{#if list.length === 0}
			<p class="hint">
				No rate stored from {from} to {end}; the latest earlier rate, if any, stays in force.
			</p>
		{:else}
			<table aria-label="{currency} rates">
				<thead>
					<tr>
						<th>Date</th>
						<th>Currency</th>
						<th class="number">Rate</th>
						<th>Source</th>
						<th>Source date</th>
					</tr>
				</thead>
				<tbody>
					{#each list as rate (rate.date)}
						<tr>
							<td>{rate.date}</td>
							<td>{rate.currency}</td>
							<td class="number">{rate.rate}</td>
							<td>{rate.source}</td>
							<td>{rate.sourceDate}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	{/each}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.alert {
		padding: 0.5rem 0.75rem;
		border: 1px solid var(--danger);
		border-radius: var(--radius);
		color: var(--danger);
	}
	.number {
		text-align: right;
		white-space: nowrap;
	}
</style>
