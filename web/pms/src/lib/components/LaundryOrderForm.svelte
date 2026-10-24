<!--
	Takes a guest laundry order for an in-house stay, or changes the lines and note of one still collected: the
	active price list by item, a quantity per service and a running total in the base currency (net of tax). An edit
	also lists the prices already on the order at the prices they were taken at, which the server keeps. A new
	order is sent with an Idempotency-Key. An edit counts against the order as it was when the form opened and sends
	that version: a stale one (412) reloads the order and starts the form again from the latest.
-->
<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { money } from '$lib/folio';
	import {
		fetchLaundryPrices,
		fetchStayLaundryOrders,
		formPrices,
		laundryKey,
		orderLines,
		orderQuantities,
		orderTotal,
		priceGroups,
		serviceLabel,
		type LaundryOrder
	} from '$lib/laundry';
	import { fetchProperties, propertiesKey } from '$lib/properties';

	interface Props {
		propertyId: string;
		reservationRoomId: string;
		roomNumber: string;
		/** The order to change, `null` for a new one. */
		orderId: string | null;
		open: boolean;
	}

	let { propertyId, reservationRoomId, roomNumber, orderId, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	const prices = createQuery(() => ({
		queryKey: laundryKey(propertyId, 'prices', 'active'),
		queryFn: ({ signal }) => fetchLaundryPrices(propertyId, false, signal),
		enabled: open
	}));
	// The stay's orders, shared with the room card, for the order being changed.
	const stayOrders = createQuery(() => ({
		queryKey: laundryKey(propertyId, 'stay', reservationRoomId),
		queryFn: ({ signal }) => fetchStayLaundryOrders(propertyId, reservationRoomId, signal),
		enabled: open && !!orderId
	}));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const baseCurrency = $derived(
		properties.data?.find((property) => property.id === propertyId)?.baseCurrency ?? ''
	);
	/** A base-currency amount, held until the property (and so its base currency) is read. */
	const base = (amount: number) => (baseCurrency ? money(amount, baseCurrency) : '…');

	let dialog = $state<HTMLDialogElement>();
	/** The order being changed as it was when the form opened; `null` for a new order. */
	let snapshot = $state.raw<{
		id: string;
		version: number;
		lines: LaundryOrder['lines'];
	} | null>(null);
	/** An edit of an order no longer collected (or gone): nothing can be changed. */
	let closed = $state(false);
	let quantities = $state<Record<string, number>>({});
	let note = $state('');
	let problem = $state('');
	let busy = $state(false);
	const keys = formKeys();

	const list = $derived(formPrices(prices.data ?? [], snapshot?.lines ?? []));
	const groups = $derived(priceGroups(list));
	const total = $derived(orderTotal(list, quantities));

	/** Starts the form from the order as it now is, or empty for a new one. */
	function reset() {
		problem = '';
		if (!orderId) {
			snapshot = null;
			closed = false;
			quantities = {};
			note = '';
			return;
		}
		const order = stayOrders.data?.find((entry) => entry.id === orderId);
		closed = order?.status !== 'COLLECTED';
		snapshot = order
			? {
					id: order.id,
					version: order.version,
					lines: order.lines
				}
			: null;
		quantities = order ? orderQuantities(order.lines) : {};
		note = order?.note ?? '';
	}

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			// The snapshot is taken here only: a refetch while the form is open must not move its version.
			untrack(reset);
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function send(lines: { price_id: string; quantity: number }[]) {
		const trimmed = note.trim();
		if (snapshot) {
			return unwrap(
				await rest.PATCH('/api/v1/properties/{property}/laundry-orders/{order}', {
					params: {
						path: { property: propertyId, order: snapshot.id },
						header: ifMatch(snapshot.version)
					},
					body: { lines, note: trimmed || null }
				})
			);
		}
		const body = {
			reservation_room_id: reservationRoomId,
			lines,
			...(trimmed ? { note: trimmed } : {})
		};
		return keys.submit(body, async (key) =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/laundry-orders', {
					params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
					body
				})
			)
		);
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		const lines = orderLines(list, quantities);
		if (lines.length === 0) {
			problem = 'Enter at least one item.';
			return;
		}
		busy = true;
		problem = '';
		try {
			await send(lines);
			open = false;
			void client.invalidateQueries({ queryKey: laundryKey(propertyId) });
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				await client.invalidateQueries({ queryKey: laundryKey(propertyId) });
				await tick();
				reset();
				problem = 'This order changed; review it again.';
			} else {
				problem = errorMessage(err);
			}
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="laundry-order-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="laundry-order-title">
			{orderId ? 'Change laundry order' : 'Laundry order'} · Room {roomNumber}
		</h2>
		{#if closed}
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<p>This order is no longer collected, so its items can't be changed.</p>
			<div class="actions">
				<button type="button" class="secondary" onclick={() => (open = false)}>Close</button>
			</div>
		{:else if prices.error && !prices.data}
			<p class="error" role="alert">{errorMessage(prices.error)}</p>
			<div class="actions">
				<button type="button" class="secondary" onclick={() => (open = false)}>Close</button>
			</div>
		{:else if !prices.data}
			<p>Loading…</p>
		{:else if groups.length === 0}
			<p>No laundry prices are set up yet.</p>
			<div class="actions">
				<button type="button" class="secondary" onclick={() => (open = false)}>Close</button>
			</div>
		{:else}
			<table aria-label="Price list">
				<thead>
					<tr>
						<th>Item</th>
						<th>Service</th>
						<th class="number">Price</th>
						<th>Quantity</th>
					</tr>
				</thead>
				{#each groups as group (group.itemCode)}
					<tbody>
						{#each group.prices as price, index (price.id)}
							<tr>
								{#if index === 0}<th scope="rowgroup" rowspan={group.prices.length}
										>{group.itemCode}</th
									>{/if}
								<td>{price.name} · {serviceLabel(price.service)}</td>
								<td class="number">{base(price.price)}</td>
								<td>
									<input
										type="number"
										min="0"
										max="100"
										aria-label="{price.name} {serviceLabel(price.service)} quantity"
										bind:value={quantities[price.id]}
									/>
								</td>
							</tr>
						{/each}
					</tbody>
				{/each}
			</table>
			<p role="status">Total <strong>{base(total)}</strong> <span class="hint">before tax</span></p>
			<label>Note <input maxlength="500" bind:value={note} /></label>
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<div class="actions">
				<button disabled={busy}>{orderId ? 'Save' : 'Take order'}</button>
				<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
			</div>
		{/if}
	</form>
</dialog>

<style>
	.number {
		text-align: right;
	}
	td input[type='number'] {
		width: 5rem;
	}
</style>
