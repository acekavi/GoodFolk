<!--
	The tape chart screen: the header (Needs a room, span, Today, paging, room picker) and the chart. The view (chosen
	rooms, page, span and first day) lives in the URL, so a reload or a bookmark reopens it; changes replace
	the history entry, so Back leaves the chart rather than undoing a scroll.
-->
<script lang="ts">
	import { goto, pushState } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { addDays } from '$lib/inventory';
	import NeedsRoom from '$lib/components/NeedsRoom.svelte';
	import ReservationModal from '$lib/components/ReservationModal.svelte';
	import RoomPicker from '$lib/components/RoomPicker.svelte';
	import TapeChart from '$lib/components/TapeChart.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchRooms, fetchRoomTypes, roomsKey, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';
	import {
		openingStart,
		PAGE_SIZE,
		pageOf,
		fetchUnassignedStays,
		overlapping,
		prefetchTapeTiles,
		railRooms,
		selectRooms,
		tilesFor,
		unassignedKey,
		unassignedWindow,
		viewFromSearchParams,
		viewToSearchParams,
		type Chip,
		type Span,
		type TapeView
	} from '$lib/tape';

	const SPANS: Span[] = [7, 14, 30];

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
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));
	const types = $derived(roomTypes.data ?? []);
	const rail = $derived(railRooms(rooms.data?.rooms ?? [], types));
	const view = $derived(
		viewFromSearchParams(
			page.url.searchParams,
			businessDate ? openingStart(businessDate) : '',
			rail,
			types
		)
	);
	const selected = $derived(selectRooms(rail, view.chips));
	const paged = $derived(pageOf(selected, view.page));
	/** Picking and paging only matter once the property has more rooms than a page. */
	const multiPage = $derived(rail.length > PAGE_SIZE);
	const position = $derived.by(() => {
		if (selected.length === 0) return 'No rooms match';
		const first = (paged.page - 1) * PAGE_SIZE + 1;
		return `Rooms ${first}–${first + paged.rooms.length - 1} of ${selected.length}`;
	});

	/** The first day in view now, which the URL may not hold yet while a scroll settles. */
	let visibleStart = $state<string>();
	let showNeeds = $state(false);

	// Read for the tiles under the view, then narrowed to the visible days, so scrolling within a tile
	// makes no request.
	const firstDay = $derived(visibleStart ?? view.start);
	const needsWindow = $derived(firstDay ? unassignedWindow(firstDay, view.span) : undefined);
	const unassigned = createQuery(() => ({
		queryKey: unassignedKey(propertyId, needsWindow?.from ?? '', needsWindow?.to ?? ''),
		queryFn: ({ signal }) =>
			fetchUnassignedStays(propertyId, needsWindow!.from, needsWindow!.to, signal),
		enabled: !!needsWindow,
		placeholderData: (previous) => previous
	}));
	const needsRoom = $derived(
		firstDay ? overlapping(unassigned.data ?? [], firstDay, addDays(firstDay, view.span)) : []
	);

	let chart = $state<{ jumpTo: (date: string) => void }>();

	// The opening tiles are requested as soon as the rooms are known, not after the chart has mounted, so
	// the first paint waits on one round trip less.
	let warmed = '';
	$effect(() => {
		if (warmed === propertyId || !view.start || paged.rooms.length === 0) return;
		warmed = propertyId;
		void prefetchTapeTiles(
			client,
			propertyId,
			tilesFor(view.start, view.span + 1, 1, 0),
			paged.rooms.map((room) => room.id)
		);
	});

	/** Today: the opening view, now, even while a scroll is settling. */
	function today() {
		if (chart) chart.jumpTo(openingStart(businessDate));
		else show({ start: openingStart(businessDate) });
	}

	/**
	 * A stay's reservation, in its modal over the chart: a shallow history entry at the reservation's URL, so
	 * the chart stays mounted with its scroll and focus, and reloading that URL opens the reservations page.
	 */
	function openReservation(reservationId: string) {
		pushState(resolve(`/p/${propertyId}/reservations/${reservationId}${page.url.search}`), {
			reservation: reservationId
		});
	}

	/** A scroll settled on `start`. Not while a reservation is open: replacing its history entry would close it. */
	function settled(start: string) {
		if (!page.state.reservation) show({ start });
	}

	/** Back to the chart's own entry; the modal unmounts with the state. */
	function closeReservation() {
		if (page.state.reservation) history.back();
	}

	function show(next: Partial<TapeView>) {
		const search = viewToSearchParams({
			...view,
			start: visibleStart ?? view.start,
			...next
		}).toString();
		void goto(resolve(`/p/${propertyId}/tape?${search}`), {
			replaceState: true,
			keepFocus: true,
			noScroll: true
		});
	}

	function turn(delta: -1 | 1) {
		const next = Math.min(Math.max(paged.page + delta, 1), paged.pages);
		if (next !== paged.page) show({ page: next });
	}

	/** Warms the cache with the tiles in view for the page `delta` away, as the chart would fetch them. */
	function prefetchPage(delta: -1 | 1) {
		const next = pageOf(selected, paged.page + delta);
		if (next.page === paged.page) return;
		const tiles = tilesFor(visibleStart ?? view.start, view.span + 1, 0, 0);
		void prefetchTapeTiles(
			client,
			propertyId,
			tiles,
			next.rooms.map((room) => room.id)
		);
	}

	function pick(chips: Chip[]) {
		show({ chips, page: 1 });
	}
</script>

<div class="title">
	<h1>Tape chart</h1>
	{#if needsRoom.length > 0}
		<button
			type="button"
			class="secondary"
			aria-expanded={showNeeds}
			onclick={() => (showNeeds = !showNeeds)}>Needs a room ({needsRoom.length})</button
		>
	{/if}
	<span class="spacer"></span>
	<div class="spans" role="group" aria-label="Days shown">
		{#each SPANS as span (span)}
			<button
				type="button"
				class="secondary"
				aria-pressed={view.span === span}
				onclick={() => show({ span })}>{span} days</button
			>
		{/each}
	</div>
	<button type="button" class="secondary" disabled={!businessDate} onclick={today}>Today</button>
	{#if multiPage}
		<span class="position" role="status">{position}</span>
		<button
			type="button"
			class="secondary"
			disabled={paged.page <= 1}
			onpointerenter={() => prefetchPage(-1)}
			onfocus={() => prefetchPage(-1)}
			onclick={() => turn(-1)}>Prev</button
		>
		<button
			type="button"
			class="secondary"
			disabled={paged.page >= paged.pages}
			onpointerenter={() => prefetchPage(1)}
			onfocus={() => prefetchPage(1)}
			onclick={() => turn(1)}>Next</button
		>
		<RoomPicker rooms={rail} {types} chips={view.chips} onchange={pick} />
	{/if}
</div>

{#if showNeeds && needsRoom.length > 0}
	<NeedsRoom {propertyId} stays={needsRoom} roomTypes={types} {manage} />
{/if}

{#if rooms.error || roomTypes.error || properties.error}
	<p class="error" role="alert">
		{errorMessage(rooms.error ?? roomTypes.error ?? properties.error)}
	</p>
{:else if !rooms.data || !roomTypes.data || !businessDate}
	<p>Loading…</p>
{:else if rail.length === 0}
	<p>No active rooms yet.</p>
{:else if paged.rooms.length === 0}
	<p>No rooms match the picker.</p>
{:else}
	{#key propertyId}
		<TapeChart
			bind:this={chart}
			{propertyId}
			rooms={paged.rooms}
			start={view.start}
			span={view.span}
			{businessDate}
			{manage}
			onview={(date) => (visibleStart = date)}
			onopen={openReservation}
			onstart={settled}
			onpage={turn}
		/>
	{/key}
{/if}

{#if page.state.reservation}
	{#key page.state.reservation}
		<ReservationModal {propertyId} id={page.state.reservation} onclose={closeReservation} />
	{/key}
{/if}

<style>
	.title {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space);
		margin-bottom: var(--space);
	}
	h1 {
		margin: 0;
	}
	.spacer {
		flex: 1;
	}
	.spans {
		display: flex;
		gap: 0.25rem;
	}
	.spans button[aria-pressed='true'] {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}
	.position {
		color: var(--muted);
	}
</style>
