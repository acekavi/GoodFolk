<!--
	The stays arriving, leaving or in house on one chosen day (a closed day or today), by room number: printable,
	and as a PDF.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import DownloadPdf from '$lib/components/DownloadPdf.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchDailyList, reportKey } from '$lib/reports';
	import { formatStay, statusLabel } from '$lib/reservations';
	import type { DailyListKind } from '$lib/api/gql/graphql';

	const KINDS: { kind: DailyListKind; label: string }[] = [
		{ kind: 'ARRIVALS', label: 'Arrivals' },
		{ kind: 'DEPARTURES', label: 'Departures' },
		{ kind: 'IN_HOUSE', label: 'In-house' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((entry) => entry.id === propertyId));

	let chosen = $state('');
	let kind = $state<DailyListKind>('ARRIVALS');
	// The property's business date until another day is chosen.
	const date = $derived(chosen || (property?.businessDate ?? ''));
	const label = $derived(KINDS.find((entry) => entry.kind === kind)?.label ?? '');

	const list = createQuery(() => ({
		queryKey: reportKey(propertyId, 'list', date, kind),
		queryFn: ({ signal }) => fetchDailyList(propertyId, date, kind, signal),
		enabled: !!date
	}));
</script>

<h1>Daily lists</h1>
<p class="no-print">
	<a href={resolve('/(app)/p/[property]/reports', { property: propertyId })}>Reports</a>
</p>

<div class="controls no-print">
	<label>
		Date
		<input
			type="date"
			bind:value={() => date, (next) => (chosen = next)}
			max={property?.businessDate}
		/>
	</label>
	<div role="tablist" aria-label="List" class="tabs">
		{#each KINDS as entry (entry.kind)}
			<button
				type="button"
				role="tab"
				aria-selected={kind === entry.kind}
				class:secondary={kind !== entry.kind}
				onclick={() => (kind = entry.kind)}>{entry.label}</button
			>
		{/each}
	</div>
</div>

{#if date}
	<div class="controls no-print">
		<button type="button" class="secondary" onclick={() => window.print()}>Print</button>
		<DownloadPdf
			{propertyId}
			request={{
				report: 'daily_list',
				params: { date, kind: kind.toLowerCase() as Lowercase<DailyListKind> }
			}}
		/>
	</div>
	<h2>{label} on {date} ({list.data?.length ?? 0})</h2>
	{#if list.error && !list.data}
		<p class="error" role="alert">{errorMessage(list.error)}</p>
	{:else if list.data?.length === 0}
		<p class="hint">No {label.toLowerCase()} on this day.</p>
	{:else if list.data}
		<table aria-label={label}>
			<thead>
				<tr>
					<th>Room</th>
					<th>Guest</th>
					<th>Booking</th>
					<th>Stay</th>
					<th>Guests</th>
					<th>Status</th>
				</tr>
			</thead>
			<tbody>
				{#each list.data as row (row.reservationRoomId)}
					<tr>
						<td>{row.roomNumber ?? 'Unassigned'}</td>
						<td>{row.guest}</td>
						<td>{row.confirmation}</td>
						<td>{formatStay(row.arrival, row.departure)}</td>
						<td
							>{row.adults} adult{row.adults === 1 ? '' : 's'}{row.children > 0
								? `, ${row.children} child${row.children === 1 ? '' : 'ren'}`
								: ''}</td
						>
						<td>{statusLabel(row.status)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{/if}

<style>
	.controls {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space);
		align-items: end;
		margin-bottom: var(--space);
	}
	.tabs {
		display: flex;
		gap: 0.25rem;
	}
	@media print {
		.no-print,
		:global(.property-nav) {
			display: none;
		}
	}
</style>
