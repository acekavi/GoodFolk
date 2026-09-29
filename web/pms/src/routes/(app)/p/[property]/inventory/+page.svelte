<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import BlockDialog from '$lib/components/BlockDialog.svelte';
	import DateGrid from '$lib/components/DateGrid.svelte';
	import {
		addDays,
		blocksOn,
		fetchMonth,
		indexInventory,
		inventoryKey,
		monthDays,
		monthOf,
		shiftMonth,
		type Block
	} from '$lib/inventory';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchRooms, fetchRoomTypes, roomsKey, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal)
	}));

	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	let chosenMonth = $state<string | null>(null);
	const month = $derived(chosenMonth ?? (businessDate ? monthOf(businessDate) : ''));
	const days = $derived(month ? monthDays(month) : []);
	const inventory = createQuery(() => ({
		queryKey: inventoryKey(propertyId, month),
		queryFn: ({ signal }) => fetchMonth(propertyId, month, signal),
		enabled: !!month,
		// The event stream invalidates a month's key when it actually changes (a block, a booking, a
		// resync), so a mount doesn't need to refetch just because 30s passed — see "queries kept fresh
		// by server events" in api-conventions.md.
		staleTime: Infinity
	}));
	const counts = $derived(indexInventory(inventory.data?.inventory ?? []));
	const rows = $derived(
		(roomTypes.data ?? [])
			.filter((type) => type.active)
			.map((type) => ({ id: type.id, label: `${type.code} · ${type.name}`, code: type.code }))
	);
	const mayBlock = $derived(!!me.data && can(me.data, 'blockRooms', propertyId));
	const monthLabel = $derived(
		month
			? new Date(`${month}-01T00:00:00Z`).toLocaleDateString(undefined, {
					month: 'long',
					year: 'numeric',
					timeZone: 'UTC'
				})
			: ''
	);

	let selected = $state<{ date: string; roomTypeId: string; code: string } | null>(null);
	const selectedBlocks = $derived.by(() => {
		if (!selected) return [];
		const typeId = selected.roomTypeId;
		const ofType = (rooms.data?.rooms ?? []).filter((room) => room.roomTypeId === typeId);
		return blocksOn(inventory.data?.blocks ?? [], selected.date, new Set(ofType.map((r) => r.id)));
	});
	let blocking = $state(false);
	let error = $state('');
	let busy = $state(false);

	const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

	function weekday(date: string): string {
		return WEEKDAYS[new Date(`${date}T00:00:00Z`).getUTCDay()];
	}

	function roomNumber(roomId: string): string {
		return rooms.data?.rooms.find((room) => room.id === roomId)?.number ?? '?';
	}

	function reasonLabel(reasonId: string): string {
		return rooms.data?.blockReasons.find((reason) => reason.id === reasonId)?.label ?? '';
	}

	function cellLabel(row: { id: string; code: string }, date: string): string {
		const day = counts.get(row.id, date);
		if (!day) return `${row.code} ${date}: not counted`;
		const over = day.sellable - Math.max(day.available, 0);
		const overbooking = over > 0 ? `, ${over} more sellable with the overbooking allowance` : '';
		return `${row.code} ${date}: ${day.available} available, ${day.sold} sold, ${day.outOfOrder} out of order${overbooking}`;
	}

	/** Ends a block as of the business date, or cancels it if it has not started. */
	async function release(block: Block) {
		busy = true;
		error = '';
		try {
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/blocks/{block}', {
					params: {
						path: { property: propertyId, block: block.id },
						header: ifMatch(block.version)
					},
					body: { to: block.from > businessDate ? block.from : businessDate }
				})
			);
			for (let m = monthOf(block.from); m <= monthOf(addDays(block.to, -1)); m = shiftMonth(m, 1)) {
				void client.invalidateQueries({ queryKey: inventoryKey(propertyId, m) });
			}
		} catch (err) {
			error = errorMessage(err);
			if (err instanceof ApiError && err.status === 412) {
				void client.invalidateQueries({ queryKey: inventoryKey(propertyId, month) });
			}
		} finally {
			busy = false;
		}
	}
</script>

<h1>Inventory</h1>
<div class="inline-form">
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
	<span class="hint">Business date: <span data-testid="business-date">{businessDate}</span></span>
	{#if mayBlock && rooms.data && roomTypes.data}
		<button onclick={() => (blocking = true)}>Block a room</button>
	{/if}
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if inventory.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(inventory.error ?? roomTypes.error)}</p>
{:else if inventory.data && roomTypes.data}
	{#if rows.length === 0}
		<p>No active room types yet.</p>
	{:else}
		<DateGrid
			label="Availability"
			{rows}
			columns={days}
			{cellLabel}
			initialColumn={Math.max(0, days.indexOf(businessDate))}
			onactivate={(row, date) => (selected = { date, roomTypeId: row.id, code: row.code })}
		>
			{#snippet header(date)}
				<span class="day" class:today={date === businessDate}>
					<small>{weekday(date)}</small>
					{Number(date.slice(8))}
				</span>
			{/snippet}
			{#snippet cell(row, date)}
				{@const day = counts.get(row.id, date)}
				{#if day}
					{@const over = day.sellable - Math.max(day.available, 0)}
					<span class="count" class:full={day.available <= 0}>{day.available}</span>
					{#if over > 0}
						<small class="hint" aria-label="{over} more sellable with the overbooking allowance"
							>+{over} over</small
						>
					{/if}
					{#if day.outOfOrder > 0}<small class="blocked">{day.outOfOrder} OOO</small>{/if}
				{:else}
					<span class="hint">–</span>
				{/if}
			{/snippet}
		</DateGrid>
		<p class="hint">
			Rooms available per room type and day. Click a day, or use the arrow keys and Enter, to see
			its blocks.
		</p>
	{/if}

	<!-- Always in the page, so a screen reader announces the blocks when Enter or a click opens a day;
	     focus stays on the grid for the next arrow key. -->
	<section aria-live="polite" aria-label={selected ? `Blocks on ${selected.date}` : undefined}>
		{#if selected}
			<h2>{selected.code} on {selected.date}</h2>
			{#if selectedBlocks.length === 0}
				<p>No rooms of this type are blocked on this day.</p>
			{:else}
				<ul>
					{#each selectedBlocks as block (block.id)}
						<li>
							Room {roomNumber(block.roomId)} · {block.kind === 'OUT_OF_ORDER'
								? 'Out of order'
								: 'Out of service'} · {reasonLabel(block.reasonId)} · {block.from} until {block.to}
							{#if block.note}· {block.note}{/if}
							{#if mayBlock && block.to > businessDate}
								<button
									class="secondary"
									disabled={busy}
									aria-label="Release room {roomNumber(block.roomId)}"
									onclick={() => release(block)}>Release</button
								>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</section>
{:else}
	<p>Loading…</p>
{/if}

{#if mayBlock && rooms.data && roomTypes.data && businessDate}
	<BlockDialog
		{propertyId}
		{businessDate}
		rooms={rooms.data.rooms}
		roomTypes={roomTypes.data}
		reasons={rooms.data.blockReasons}
		from={selected?.date ?? businessDate}
		bind:open={blocking}
	/>
{/if}

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
	.count {
		font-weight: 600;
	}
	.count.full,
	.blocked {
		color: var(--danger);
	}
</style>
