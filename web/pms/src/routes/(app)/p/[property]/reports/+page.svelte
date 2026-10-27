<!--
	The everyday report of a range of closed business dates: the headline figures with the same period last year,
	each day, and the breakdowns, with Download PDF. Days not closed yet are listed as such and never estimated.
	The signed-in user's own weekly and monthly report emails of the property are switched here, as there is no
	per-user settings page.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import DownloadPdf from '$lib/components/DownloadPdf.svelte';
	import { addDays } from '$lib/inventory';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import {
		datesBetween,
		dimensionLabel,
		fetchReportEmails,
		fetchReportSummary,
		formatLkr,
		formatOccupancy,
		keyLabel,
		presets,
		reportEmailsKey,
		reportKey,
		setReportEmail
	} from '$lib/reports';

	const MAX_DAYS = 366;

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((entry) => entry.id === propertyId));
	const ranges = $derived(property ? presets(property.businessDate) : []);

	let rangeId = $state('last-week');
	let customFrom = $state('');
	let customTo = $state('');
	const preset = $derived(ranges.find((entry) => entry.id === rangeId));
	const from = $derived(rangeId === 'custom' ? customFrom : (preset?.from ?? ''));
	const to = $derived(rangeId === 'custom' ? customTo : (preset?.to ?? ''));
	const rangeProblem = $derived.by(() => {
		if (rangeId !== 'custom' || !from || !to) return '';
		if (to < from) return 'The range ends before it starts.';
		if (addDays(from, MAX_DAYS) <= to) return `A range is at most ${MAX_DAYS} days.`;
		return '';
	});
	const valid = $derived(!!from && !!to && !rangeProblem);

	const summary = createQuery(() => ({
		queryKey: reportKey(propertyId, 'summary', from, to),
		queryFn: ({ signal }) => fetchReportSummary(propertyId, from, to, signal),
		enabled: valid
	}));
	const data = $derived(summary.data);
	const rows = $derived(
		data
			? datesBetween(from, to).map((date) => ({
					date,
					day: data.days.find((entry) => entry.businessDate === date)
				}))
			: []
	);
	type Figures = NonNullable<typeof data>['headline'];
	/** The headline cards of a period's figures, in order. */
	const cards = (figures: Figures) => [
		{ label: 'Occupancy', value: formatOccupancy(figures.occupancyBp) },
		{ label: 'ADR', value: formatLkr(figures.adr) },
		{ label: 'RevPAR', value: formatLkr(figures.revpar) },
		{ label: 'Room nights sold', value: String(figures.roomNights) },
		{ label: 'Total revenue', value: formatLkr(figures.totalRevenue) }
	];

	const emails = createQuery(() => ({
		queryKey: reportEmailsKey(propertyId),
		queryFn: ({ signal }) => fetchReportEmails(propertyId, signal)
	}));
	const EMAIL_KINDS = [
		{ kind: 'WEEKLY', label: 'Weekly', send: 'weekly' },
		{ kind: 'MONTHLY', label: 'Monthly', send: 'monthly' }
	] as const;
	const pending = new Pending();
	let emailProblem = $state('');

	async function switchEmail(send: 'weekly' | 'monthly', enabled: boolean) {
		emailProblem = '';
		try {
			await pending.run(send, () => setReportEmail(propertyId, send, enabled));
		} catch (err) {
			emailProblem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: reportEmailsKey(propertyId) });
		}
	}
</script>

<h1>Reports</h1>
<p>
	<a href={resolve('/(app)/p/[property]/reports/lists', { property: propertyId })}>Daily lists</a>
</p>

<section aria-labelledby="range">
	<h2 id="range">Range</h2>
	<div class="ranges" role="group" aria-label="Range">
		{#each ranges as range (range.id)}
			<button
				type="button"
				class:secondary={rangeId !== range.id}
				aria-pressed={rangeId === range.id}
				onclick={() => (rangeId = range.id)}>{range.label}</button
			>
		{/each}
		<button
			type="button"
			class:secondary={rangeId !== 'custom'}
			aria-pressed={rangeId === 'custom'}
			onclick={() => (rangeId = 'custom')}>Custom</button
		>
	</div>
	{#if rangeId === 'custom'}
		<div class="ranges">
			<label>From <input type="date" bind:value={customFrom} /></label>
			<label>To <input type="date" bind:value={customTo} /></label>
		</div>
	{/if}
	{#if rangeProblem}<p class="error" role="alert">{rangeProblem}</p>{/if}
	{#if valid}<p class="hint">{from} – {to}</p>{/if}
</section>

{#if summary.error && !data}
	<p class="error" role="alert">{errorMessage(summary.error)}</p>
{:else if data}
	{#if data.notClosed.length > 0}
		<p role="status" class="notice">
			{#if data.closedFrom && data.closedTo}
				Not closed yet: figures cover {data.closedFrom}–{data.closedTo}
			{:else}
				Not closed yet: no day of this range is closed
			{/if}
		</p>
	{/if}
	<div class="actions">
		<DownloadPdf {propertyId} request={{ report: 'summary', params: { from, to } }} />
	</div>
	<p class="hint">Revenue is net of tax, in LKR.</p>

	<section aria-labelledby="headline">
		<h2 id="headline">Headline figures</h2>
		<ul class="cards">
			{#each cards(data.headline) as card, index (card.label)}
				<li>
					<span class="label">{card.label}</span>
					<strong>{card.value}</strong>
					{#if data.lastYear}
						<span class="hint">Last year: {cards(data.lastYear)[index].value}</span>
					{/if}
				</li>
			{/each}
		</ul>
	</section>

	<section aria-labelledby="days">
		<h2 id="days">Days</h2>
		<div class="scroll">
			<table>
				<thead>
					<tr>
						<th>Date</th>
						<th>Occupancy</th>
						<th>Rooms sold</th>
						<th>Guests</th>
						<th>Arrivals</th>
						<th>Departures</th>
						<th>Rooms</th>
						<th>Meal plans</th>
						<th>POS</th>
						<th>Fees</th>
						<th>Other</th>
						<th>Total</th>
						<th>ADR</th>
						<th>RevPAR</th>
					</tr>
				</thead>
				<tbody>
					{#each rows as { date, day } (date)}
						<tr>
							<td>{date}</td>
							{#if day}
								<td>{formatOccupancy(day.occupancyBp)}</td>
								<td>{day.roomsSold}/{day.roomsAvailable}</td>
								<td>{day.adults + day.children}</td>
								<td>{day.arrivals}</td>
								<td>{day.departures}</td>
								<td>{formatLkr(day.roomRevenue)}</td>
								<td>{formatLkr(day.mealPlanRevenue)}</td>
								<td>{formatLkr(day.posRevenue)}</td>
								<td>{formatLkr(day.feeRevenue)}</td>
								<td>{formatLkr(day.otherRevenue)}</td>
								<td>{formatLkr(day.totalRevenue)}</td>
								<td>{formatLkr(day.adr)}</td>
								<td>{formatLkr(day.revpar)}</td>
							{:else}
								<td colspan="13" class="hint">Not closed yet</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</section>

	{#each data.breakdowns as breakdown (breakdown.dimension)}
		{@const label = dimensionLabel(breakdown.dimension)}
		<section aria-labelledby="by-{breakdown.dimension}">
			<h2 id="by-{breakdown.dimension}">By {label.toLowerCase()}</h2>
			<div class="scroll">
				<table aria-label="Revenue by {label.toLowerCase()}">
					<thead>
						<tr>
							<th>{label}</th>
							<th>Room nights</th>
							<th>Room revenue</th>
							<th>Total revenue</th>
						</tr>
					</thead>
					<tbody>
						{#each breakdown.keys as entry (entry.key)}
							<tr>
								<td>{keyLabel(entry.key)}</td>
								<td>{entry.roomNights}</td>
								<td>{formatLkr(entry.roomRevenue)}</td>
								<td>{formatLkr(entry.revenue)}</td>
							</tr>
						{:else}
							<tr><td colspan="4" class="hint">Nothing in the closed days.</td></tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>
	{/each}
{:else if valid}
	<p class="hint" role="status">Loading…</p>
{/if}

<section aria-labelledby="emails">
	<h2 id="emails">Report emails for {property?.name ?? ''}</h2>
	<p class="hint">
		The weekly report goes out after Sunday closes, the monthly one after the month's last day
		closes. They go to the property's owners and managers, and only if you have them on.
	</p>
	{#if emails.error && !emails.data}
		<p class="error" role="alert">{errorMessage(emails.error)}</p>
	{:else if emails.data}
		{#each EMAIL_KINDS as entry (entry.kind)}
			<label class="switch">
				<input
					type="checkbox"
					checked={emails.data.find((email) => email.kind === entry.kind)?.enabled ?? true}
					disabled={pending.has(entry.send)}
					onchange={(event) => switchEmail(entry.send, event.currentTarget.checked)}
				/>
				{entry.label}
			</label>
		{/each}
	{/if}
	{#if emailProblem}<p class="error" role="alert">{emailProblem}</p>{/if}
</section>

<style>
	.ranges {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space);
		align-items: end;
		margin-bottom: var(--space);
	}
	.actions {
		margin-bottom: var(--space);
	}
	.notice {
		padding: 0.5rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
		gap: var(--space);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.cards li {
		display: flex;
		flex-direction: column;
		padding: 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.cards strong {
		font-size: 1.25rem;
	}
	.switch {
		display: block;
	}
	.scroll {
		overflow-x: auto;
	}
</style>
