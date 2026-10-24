<!--
	Guest laundry, for `LaundryOrders`: the open orders by status (Collected, Processing, Ready) and those delivered
	today, each card with one button for its next step and Cancel before delivery. Delivering posts the order's
	charge to the stay's folio, so it is confirmed first. Orders are taken from the room's card in the reservation
	modal. Every query here refetches on the server's `laundry:<property>` event.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { money } from '$lib/folio';
	import {
		boardColumns,
		fetchLaundryOrders,
		laundryKey,
		lineText,
		localDate,
		nextStep,
		type LaundryOrder,
		type OrderStatusValue
	} from '$lib/laundry';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const orders = createQuery(() => ({
		queryKey: laundryKey(propertyId, 'orders'),
		queryFn: ({ signal }) => fetchLaundryOrders(propertyId, signal)
	}));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((entry) => entry.id === propertyId));
	const columns = $derived(
		property
			? boardColumns(orders.data ?? [], localDate(new Date(), property.timezone), property.timezone)
			: []
	);

	const pending = new Pending();
	let problem = $state('');

	/**
	 * Moves the order to `status`, sending the version on its card. A 412 means it changed since it was shown: the
	 * board refetches for the person to look again rather than retry.
	 */
	async function move(order: LaundryOrder, status: OrderStatusValue) {
		problem = '';
		try {
			await pending.run(order.id, async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/laundry-orders/{order}/status', {
						params: {
							path: { property: propertyId, order: order.id },
							header: ifMatch(order.version)
						},
						body: { status }
					})
				)
			);
		} catch (err) {
			problem =
				err instanceof ApiError && err.status === 412
					? 'This order changed; review it again.'
					: errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: laundryKey(propertyId) });
		}
	}

	function step(order: LaundryOrder) {
		const next = nextStep(order.status);
		if (!next) return;
		if (
			next.status === 'delivered' &&
			!confirm(`Post ${money(order.total, order.currency)} to Room ${order.roomNumber}'s folio?`)
		)
			return;
		void move(order, next.status);
	}

	function cancel(order: LaundryOrder) {
		if (!confirm(`Cancel Room ${order.roomNumber}'s laundry order?`)) return;
		void move(order, 'cancelled');
	}

	function at(time: string) {
		return new Date(time).toLocaleTimeString(undefined, { timeStyle: 'short' });
	}
</script>

<h1>Guest laundry</h1>
<p class="hint">Orders are taken from a checked-in room in the reservation.</p>
{#if problem}<p class="error" role="alert">{problem}</p>{/if}

{#if orders.error && !orders.data}
	<p class="error" role="alert">{errorMessage(orders.error)}</p>
	<button onclick={() => orders.refetch()}>Retry</button>
{:else if properties.error && !property}
	<p class="error" role="alert">{errorMessage(properties.error)}</p>
{:else if !orders.data || !property}
	<p>Loading…</p>
{:else}
	<div class="board">
		{#each columns as column (column.id)}
			<section class="column" aria-labelledby="laundry-{column.id}">
				<h2 id="laundry-{column.id}">
					{column.label} <span class="count">{column.orders.length}</span>
				</h2>
				{#each column.orders as order (order.id)}
					{@const next = nextStep(order.status)}
					<article class="card" aria-label="Room {order.roomNumber} laundry">
						<h3>Room {order.roomNumber}</h3>
						<p>{order.guest}</p>
						<ul>
							{#each order.lines as line (line.position)}
								<li>{lineText(line)}</li>
							{/each}
						</ul>
						{#if order.note}<p class="hint">{order.note}</p>{/if}
						<p>
							<strong>{money(order.total, order.currency)}</strong>
							<span class="hint">· {at(order.deliveredAt ?? order.statusAt)}</span>
						</p>
						{#if next}
							<div class="actions">
								<button disabled={pending.has(order.id)} onclick={() => step(order)}
									>{next.label}</button
								>
								<button
									class="secondary"
									disabled={pending.has(order.id)}
									onclick={() => cancel(order)}>Cancel</button
								>
							</div>
						{/if}
					</article>
				{:else}
					<p class="hint">None.</p>
				{/each}
			</section>
		{/each}
	</div>
{/if}

<style>
	.board {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr));
		gap: var(--space);
		align-items: start;
	}
	.column h2 {
		font-size: 1rem;
	}
	.count {
		color: var(--muted);
		font-weight: normal;
	}
	.card {
		margin-bottom: var(--space);
		padding: 0.5rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.card h3 {
		margin: 0;
		font-size: 1rem;
	}
	.card p,
	.card ul {
		margin: 0.25rem 0;
	}
	.card ul {
		padding-left: 1.1rem;
	}
</style>
