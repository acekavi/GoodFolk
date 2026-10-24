<!--
	The property's past closes (date, mode, who, duration and status, each with its step reports) and the
	statistics of the last 30 closed dates. Both refetch on the server's `audit:<property>` events.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { money } from '$lib/folio';
	import { addDays } from '$lib/inventory';
	import {
		auditRunsKey,
		dailyStatsKey,
		dayRevenue,
		failedStays,
		fetchAuditRuns,
		fetchDailyStats,
		occupancyText,
		runDuration,
		stepDetailText,
		stepLabel
	} from '$lib/night-audit';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchMe } from '$lib/session';

	const RUNS = 30;
	const DAYS = 30;

	const propertyId = $derived(page.params.property ?? '');
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((entry) => entry.id === propertyId));
	const baseCurrency = $derived(property?.baseCurrency ?? '');
	// The closed dates are those before the business date.
	const to = $derived(property ? addDays(property.businessDate, -1) : '');
	const from = $derived(to ? addDays(to, 1 - DAYS) : '');

	/** A base-currency amount, held until the property (and so its base currency) is read. */
	const base = (amount: number) => (baseCurrency ? money(amount, baseCurrency) : '…');

	const runs = createQuery(() => ({
		queryKey: auditRunsKey(propertyId, RUNS),
		queryFn: ({ signal }) => fetchAuditRuns(propertyId, RUNS, signal)
	}));
	const stats = createQuery(() => ({
		queryKey: dailyStatsKey(propertyId, from, to),
		queryFn: ({ signal }) => fetchDailyStats(propertyId, from, to, signal),
		enabled: !!to
	}));
	const days = $derived(
		[...(stats.data ?? [])].sort((a, b) => b.businessDate.localeCompare(a.businessDate))
	);

	const MODE_LABELS = { MANUAL: 'Manual', AUTO: 'Automatic' } as const;
	const STATUS_LABELS = { RUNNING: 'Running', FAILED: 'Failed', COMPLETED: 'Completed' } as const;

	function who(startedBy: string | null | undefined): string {
		if (!startedBy) return 'System';
		return startedBy === me.data?.user_id ? 'You' : 'Staff';
	}

	function when(at: string): string {
		return new Date(at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}
</script>

<h1>Audit history</h1>

<section aria-labelledby="runs">
	<h2 id="runs">Closes ({runs.data?.length ?? 0})</h2>
	{#if runs.error && !runs.data}
		<p class="error" role="alert">{errorMessage(runs.error)}</p>
	{:else if runs.data?.length === 0}
		<p class="hint">No day has been closed yet.</p>
	{:else if runs.data}
		<div class="scroll">
			<table>
				<thead>
					<tr>
						<th>Date</th>
						<th>Mode</th>
						<th>Who</th>
						<th>Duration</th>
						<th>Status</th>
						<th>Steps</th>
					</tr>
				</thead>
				<tbody>
					{#each runs.data as run (run.id)}
						{@const stays = failedStays(run)}
						<tr class:failed={run.status === 'FAILED'}>
							<td>{run.businessDate}</td>
							<td>{MODE_LABELS[run.mode]}</td>
							<td>{who(run.startedBy)}</td>
							<td>{runDuration(run) ?? '…'}</td>
							<td>
								{STATUS_LABELS[run.status]}{run.attempts > 1 ? ` (${run.attempts} attempts)` : ''}
							</td>
							<td>
								<details>
									<summary>{run.steps.length} of 5 steps</summary>
									<p class="hint">Started {when(run.startedAt)}</p>
									<ol>
										{#each run.steps as step (step.step)}
											<li>
												{stepLabel(step.step)} · {when(step.completedAt)}
												{#if stepDetailText(step.detail)}
													<span class="hint">({stepDetailText(step.detail)})</span>
												{/if}
											</li>
										{/each}
									</ol>
									{#if run.error}<p class="error">{run.error}</p>{/if}
									{#if stays.length > 0}
										<ul>
											{#each stays as stay, index (index)}
												<li>Room {stay.room ?? 'unassigned'}: {stay.error}</li>
											{/each}
										</ul>
									{/if}
								</details>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<section aria-labelledby="days">
	<h2 id="days">Last {DAYS} days ({days.length})</h2>
	{#if stats.error && !stats.data}
		<p class="error" role="alert">{errorMessage(stats.error)}</p>
	{:else if stats.data && days.length === 0}
		<p class="hint">No closed day in the last {DAYS} days.</p>
	{:else if stats.data}
		<p class="hint">Revenue is net of tax{baseCurrency ? `, in ${baseCurrency}` : ''}.</p>
		<div class="scroll">
			<table>
				<thead>
					<tr>
						<th>Date</th>
						<th>Occupancy</th>
						<th>ADR</th>
						<th>RevPAR</th>
						<th>Revenue</th>
						<th>Arrivals</th>
						<th>Departures</th>
					</tr>
				</thead>
				<tbody>
					{#each days as day (day.businessDate)}
						<tr>
							<td>{day.businessDate}</td>
							<td>{occupancyText(day.occupancyBp)} ({day.roomsSold}/{day.roomsAvailable})</td>
							<td>{base(day.adr)}</td>
							<td>{base(day.revpar)}</td>
							<td>{base(dayRevenue(day))}</td>
							<td>{day.arrivals}</td>
							<td>{day.departures}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<style>
	.scroll {
		overflow-x: auto;
	}
	tr.failed td {
		color: var(--danger);
	}
	details ol,
	details ul {
		margin: 0.25rem 0;
		padding-left: 1.25rem;
	}
</style>
