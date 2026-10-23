<!--
	The night audit of the property's business date: the five review lists, the row actions that resolve each
	arrival and departure (confirmed in `AuditActionDialog`; Cancel opens the reservation and its own cancel), and
	Close the day once nothing is left to resolve. The close runs in the background: the step tracker follows the
	run, which the server's `audit:<property>` events refetch, and the review reloads for the new date when it
	completes.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { rest, unwrap } from '$lib/api/rest';
	import AuditActionDialog from '$lib/components/AuditActionDialog.svelte';
	import AuditStepTracker from '$lib/components/AuditStepTracker.svelte';
	import ReservationModal from '$lib/components/ReservationModal.svelte';
	import { money } from '$lib/folio';
	import {
		auditRunsKey,
		blockingText,
		fetchAuditRuns,
		fetchNightAudit,
		nightAuditKey,
		nightPosting,
		owedText,
		paymentsByMethod,
		type AuditAction
	} from '$lib/night-audit';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { formatStay } from '$lib/reservations';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const baseCurrency = $derived(
		properties.data?.find((property) => property.id === propertyId)?.baseCurrency ?? ''
	);
	/** A base-currency amount, held until the property (and so its base currency) is read. */
	const base = (amount: number) => (baseCurrency ? money(amount, baseCurrency) : '…');
	const review = createQuery(() => ({
		queryKey: nightAuditKey(propertyId),
		queryFn: ({ signal }) => fetchNightAudit(propertyId, signal)
	}));
	// The newest run: the close under way, or the last one.
	const runs = createQuery(() => ({
		queryKey: auditRunsKey(propertyId, 1),
		queryFn: ({ signal }) => fetchAuditRuns(propertyId, 1, signal)
	}));
	const latest = $derived(runs.data?.[0]);
	const data = $derived(review.data);

	/** The run started here; a run of the date under way or failed is followed without it. */
	let started = $state<string | null>(null);
	const tracked = $derived(
		latest &&
			(latest.id === started ||
				(latest.status !== 'COMPLETED' && latest.businessDate === data?.businessDate))
			? latest
			: null
	);
	const running = $derived(tracked?.status === 'RUNNING');

	const pending = new Pending();
	let problem = $state('');
	let notice = $state('');
	let action = $state<AuditAction | null>(null);
	let cancelling = $state<string | null>(null);

	// Once the followed run completes, the review is of a closed date: read the new one.
	let reloadedFor = '';
	$effect(() => {
		if (tracked?.status !== 'COMPLETED' || reloadedFor === tracked.id) return;
		if (data?.businessDate !== tracked.businessDate) return;
		reloadedFor = tracked.id;
		void client.invalidateQueries({ queryKey: nightAuditKey(propertyId) });
		void client.invalidateQueries({ queryKey: propertiesKey });
	});

	async function closeDay() {
		problem = '';
		notice = '';
		try {
			const done = await pending.run('close', async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/night-audit/close', {
						params: { path: { property: propertyId } }
					})
				)
			);
			started = done.run_id;
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: auditRunsKey(propertyId) });
		}
	}

	async function retry(run: string) {
		problem = '';
		try {
			await pending.run('retry', async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/night-audit/runs/{run}/retry', {
						params: { path: { property: propertyId, run } }
					})
				)
			);
			started = run;
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: auditRunsKey(propertyId) });
		}
	}

	function open(next: AuditAction) {
		problem = '';
		notice = '';
		action = next;
	}

	function done(result: { notice?: string; problem?: string }) {
		notice = result.notice ?? '';
		problem = result.problem ?? '';
	}

	function closedReservation() {
		cancelling = null;
		void client.invalidateQueries({ queryKey: nightAuditKey(propertyId) });
	}

	const posBillCount = $derived(
		data?.posBills.reduce((sum, outlet) => sum + outlet.bills.length, 0) ?? 0
	);
	const paymentCount = $derived(data?.payments.reduce((sum, group) => sum + group.count, 0) ?? 0);
	const methods = $derived(paymentsByMethod(data?.payments ?? []));
</script>

<h1>Night audit</h1>
<p>
	<a href={resolve('/(app)/p/[property]/night-audit/history', { property: propertyId })}
		>Audit history</a
	>
</p>

{#if review.error && !data}
	<p class="error" role="alert">{errorMessage(review.error)}</p>
	<button onclick={() => review.refetch()}>Retry</button>
{:else if data}
	<div class="header">
		<p>Business date <strong>{data.businessDate}</strong></p>
		<p>{blockingText(data.blocking)}</p>
		<button disabled={data.blocking > 0 || running || pending.has('close')} onclick={closeDay}
			>Close the day</button
		>
	</div>
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}
	{#if notice}<p role="status">{notice}</p>{/if}

	{#if tracked}
		<AuditStepTracker
			run={tracked}
			retrying={pending.has('retry')}
			onretry={tracked.status === 'FAILED' ? () => retry(tracked.id) : undefined}
		/>
	{/if}

	<section aria-labelledby="arrivals">
		<h2 id="arrivals">Arrivals not checked in ({data.arrivals.length})</h2>
		{#if data.arrivals.length === 0}
			<p class="hint">Every arrival is checked in or resolved.</p>
		{:else}
			<div class="scroll">
				<table>
					<thead>
						<tr>
							<th>Guest</th>
							<th>Booking</th>
							<th>Room</th>
							<th>Stay</th>
							<th>Total</th>
							<th><span class="visually-hidden">Actions</span></th>
						</tr>
					</thead>
					<tbody>
						{#each data.arrivals as row (row.reservationRoomId)}
							<tr>
								<td>
									{row.guest}
									{#if row.status === 'TENTATIVE'}<span class="tag">Tentative</span>{/if}
									{#if row.arrival < data.businessDate}<span class="tag">Overdue</span>{/if}
								</td>
								<td>{row.confirmation}</td>
								<td>{row.roomNumber ?? 'Unassigned'}</td>
								<td>{formatStay(row.arrival, row.departure)}</td>
								<td>{money(row.total, row.currency)}</td>
								<td class="actions">
									{#if row.status === 'CONFIRMED'}
										<button onclick={() => open({ kind: 'noShow', row })}>No-show</button>
									{/if}
									<button class="secondary" onclick={() => open({ kind: 'delay', row })}
										>Delay arrival</button
									>
									<button class="secondary" onclick={() => (cancelling = row.reservationId)}
										>Cancel</button
									>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>

	<section aria-labelledby="departures">
		<h2 id="departures">Departures not checked out ({data.departures.length})</h2>
		{#if data.departures.length === 0}
			<p class="hint">Every departure is checked out.</p>
		{:else}
			<div class="scroll">
				<table>
					<thead>
						<tr>
							<th>Guest</th>
							<th>Booking</th>
							<th>Room</th>
							<th>Departure</th>
							<th>Balance</th>
							<th><span class="visually-hidden">Actions</span></th>
						</tr>
					</thead>
					<tbody>
						{#each data.departures as row (row.reservationRoomId)}
							<tr>
								<td>
									{row.guest}
									{#if row.departure < data.businessDate}<span class="tag">Overdue</span>{/if}
								</td>
								<td>{row.confirmation}</td>
								<td>{row.roomNumber ?? 'Unassigned'}</td>
								<td>{row.departure}</td>
								<td>
									{owedText(row.balances) ?? 'Settled'}
								</td>
								<td class="actions">
									<button onclick={() => open({ kind: 'checkOut', row })}>Check out</button>
									<!-- Only a stay leaving on the business date can stay one more night. -->
									{#if row.departure === data.businessDate}
										<button class="secondary" onclick={() => open({ kind: 'extend', row })}
											>Extend a night</button
										>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>

	<section aria-labelledby="pos-bills">
		<h2 id="pos-bills">POS bills ({posBillCount})</h2>
		{#if data.posBills.length === 0}
			<p class="hint">No POS bills today.</p>
		{/if}
		{#each data.posBills as outlet (outlet.outlet)}
			<h3>{outlet.outlet} ({outlet.bills.length})</h3>
			<div class="scroll">
				<table>
					<thead>
						<tr><th>Bill</th><th>Total</th><th>Posted to</th></tr>
					</thead>
					<tbody>
						{#each outlet.bills as bill (bill.id)}
							<tr class:highlight={bill.highlight}>
								<td>{bill.billNo}</td>
								<td>{money(bill.total, bill.currency)}</td>
								<td>
									{bill.settled
										? 'Settled at the outlet'
										: bill.roomNumber
											? `Room ${bill.roomNumber}`
											: 'A stay'}
									{#if bill.highlight}<span class="tag">Checked out unsettled</span>{/if}
								</td>
							</tr>
						{/each}
					</tbody>
					<tfoot>
						<tr>
							<th scope="row">Total</th>
							<td colspan="2">{base(outlet.totalBase)}</td>
						</tr>
					</tfoot>
				</table>
			</div>
		{/each}
	</section>

	<section aria-labelledby="booking-bills">
		<h2 id="booking-bills">Booking bills ({data.bookingBills.nights.length})</h2>
		<h3>Tonight's room and meal-plan charges</h3>
		{#if data.bookingBills.nights.length === 0}
			<p class="hint">No stay in house tonight.</p>
		{:else}
			<div class="scroll">
				<table>
					<thead>
						<tr><th>Room</th><th>Night</th><th>Room</th><th>Meal plan</th><th>Status</th></tr>
					</thead>
					<tbody>
						{#each data.bookingBills.nights as night (night.reservationRoomId)}
							<tr class:highlight={!!night.problem}>
								<td>{night.roomNumber ?? 'Unassigned'}</td>
								<td>{night.date}</td>
								<td>{money(night.roomAmount, night.currency)}</td>
								<td>{money(night.mealAmount, night.currency)}</td>
								<td>
									{#if night.problem}
										<span class="error">{night.problem}</span>
									{:else if nightPosting(night) === 'posted'}
										Already posted
									{:else if nightPosting(night) === 'partly'}
										Partly posted
									{:else}
										To post
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
		<h3>Manual and fee lines posted today</h3>
		{#if data.bookingBills.posted.length === 0}
			<p class="hint">None.</p>
		{:else}
			<div class="scroll">
				<table>
					<thead><tr><th>Category</th><th>Lines</th><th>Total</th></tr></thead>
					<tbody>
						{#each data.bookingBills.posted as category (category.category)}
							<tr>
								<td>{category.category}</td>
								<td>{category.lines}</td>
								<td>{base(category.totalBase)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>

	<section aria-labelledby="payments">
		<h2 id="payments">Payments ({paymentCount})</h2>
		{#if methods.length === 0}
			<p class="hint">No payments today.</p>
		{/if}
		{#each methods as method (method.method)}
			<h3>{method.label}</h3>
			<div class="scroll">
				<table>
					<thead>
						<tr>
							<th>Currency</th>
							<th>Payments</th>
							<th>Amount</th>
							<th>In {baseCurrency || 'base'}</th>
							<th>Refunds</th>
							<th>Flags</th>
						</tr>
					</thead>
					<tbody>
						{#each method.groups as group (group.currency)}
							<tr class:highlight={group.flagged.length > 0}>
								<td>{group.currency}</td>
								<td>{group.count}</td>
								<td>{money(group.amount, group.currency)}</td>
								<td>{base(group.baseAmount)}</td>
								<td>{money(group.refunds, group.currency)}</td>
								<td>
									{group.flagged.length === 0
										? ''
										: `${group.flagged.length} non-resident LKR payment${group.flagged.length === 1 ? '' : 's'} without evidence`}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/each}
	</section>

	<AuditActionDialog {propertyId} businessDate={data.businessDate} bind:action ondone={done} />
{/if}

{#if cancelling}
	<ReservationModal {propertyId} id={cancelling} onclose={closedReservation} />
{/if}

<style>
	.header {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space);
		align-items: baseline;
		margin-bottom: var(--space);
	}
	.header p {
		margin: 0;
	}
	.scroll {
		overflow-x: auto;
	}
	.tag {
		margin-left: 0.25rem;
		padding: 0 0.35rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		color: var(--muted);
		font-size: 0.85em;
	}
	tr.highlight td {
		color: var(--danger);
	}
</style>
