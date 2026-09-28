<!--
	The reservations table. It is this layout, not a page, so that the reservation modal at `[id]` opens over
	it: SvelteKit keeps a layout mounted while only its child page changes, so the table keeps its loaded
	pages and its scroll position while a reservation is open, and after Back.

	Rows have a fixed height and only those in view (plus overscan) are in the DOM; the scroller is as tall
	as every loaded row. Pages come from the server's cursor, the next one fetched as the end comes near.
	Filter and sort live in the URL's search params. ArrowUp and ArrowDown move between the row links.
	Closing a reservation's modal puts focus back on the row that opened it, if it is still rendered.
-->
<script lang="ts">
	import { afterNavigate, goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createInfiniteQuery, createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { tick, untrack } from 'svelte';
	import type { ReservationSortField } from '$lib/api/gql/graphql';
	import { errorMessage } from '$lib/api/problem';
	import { revealRow, visibleRows } from '$lib/grid';
	import { formatMoney } from '$lib/rates';
	import {
		fetchReservation,
		fetchReservations,
		filterFromSearchParams,
		filterToSearchParams,
		reservationKey,
		reservationsKey,
		SOURCES,
		sourceLabel,
		STATUSES,
		statusLabel,
		toggleChoice,
		type ReservationFilter,
		type ReservationRoomRow
	} from '$lib/reservations';
	import { can, fetchMe } from '$lib/session';

	let { children } = $props();

	const PAGE_SIZE = 50;
	const ROW_HEIGHT = 36;
	const OVERSCAN = 5;
	/** The next page is fetched once the rows in view come this close to the last loaded one. */
	const LOAD_AHEAD = 20;
	const SEARCH_DELAY_MS = 300;
	const COLUMNS: { label: string; sort?: ReservationSortField }[] = [
		{ label: 'Confirmation #', sort: 'CONFIRMATION' },
		{ label: 'Guest', sort: 'GUEST' },
		{ label: 'Arrival', sort: 'ARRIVAL' },
		{ label: 'Departure' },
		{ label: 'Nights' },
		{ label: 'Room type / Room' },
		{ label: 'Status' },
		{ label: 'Source' },
		{ label: 'Total' },
		{ label: 'Account' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));

	const params = $derived(filterFromSearchParams(page.url.searchParams));
	const filtered = $derived(Object.keys(params.filter).length > 0);
	const list = createInfiniteQuery(() => ({
		queryKey: reservationsKey(propertyId, params),
		queryFn: ({ pageParam, signal }) =>
			fetchReservations(propertyId, params, PAGE_SIZE, pageParam, signal),
		initialPageParam: undefined as string | undefined,
		getNextPageParam: (last) =>
			last.pageInfo.hasNextPage ? (last.pageInfo.endCursor ?? undefined) : undefined
	}));
	const rows = $derived(list.data?.pages.flatMap((result) => result.nodes) ?? []);
	const total = $derived(list.data?.pages[0]?.totalCount ?? 0);

	let scroller = $state<HTMLDivElement>();
	let scrollTop = $state(0);
	let height = $state(0);
	// The row whose link takes Tab (a roving tab stop), kept rendered wherever the list is scrolled.
	let active = $state(0);
	const focusRow = $derived(Math.min(active, rows.length - 1));
	const range = $derived(
		visibleRows(scrollTop, Math.max(0, height - ROW_HEIGHT), ROW_HEIGHT, rows.length, OVERSCAN)
	);
	const rendered = $derived.by(() => {
		const inView = Array.from({ length: range.end - range.start }, (_, i) => range.start + i);
		const outside = focusRow >= 0 && (focusRow < range.start || focusRow >= range.end);
		return outside ? [focusRow, ...inView] : inView;
	});

	// Fetch the next page before the rows in view reach the end of what is loaded. A failed page waits for
	// Retry rather than being fetched again on every scroll.
	$effect(() => {
		if (
			range.end >= rows.length - LOAD_AHEAD &&
			list.hasNextPage &&
			!list.isFetchingNextPage &&
			!list.isFetchNextPageError
		) {
			void list.fetchNextPage();
		}
	});

	// A new filter or sort is a new list: back to its top. Opening a reservation (same params) is not.
	const listKey = $derived(JSON.stringify(reservationsKey(propertyId, params)));
	$effect(() => {
		void listKey;
		untrack(() => {
			active = 0;
			// The scroller is remounted while the new list loads, so the state is reset along with it.
			scrollTop = 0;
			if (scroller) scroller.scrollTop = 0;
		});
	});

	/** Shows the list for `filter` and `sort`, replacing the history entry so Back leaves the table. */
	function apply(filter: ReservationFilter, sort = params.sort) {
		const search = filterToSearchParams({ filter, sort }).toString();
		void goto(
			search
				? resolve(`/p/${propertyId}/reservations?${search}`)
				: resolve('/(app)/p/[property]/reservations/(list)', { property: propertyId }),
			{ replaceState: true, keepFocus: true, noScroll: true }
		);
	}

	function sortBy(field: ReservationSortField) {
		const direction =
			params.sort.field === field && params.sort.direction === 'ASC' ? 'DESC' : 'ASC';
		apply(params.filter, { field, direction });
	}

	// The search box is typed into directly and reaches the URL after a pause. `pushed` is the text last
	// sent to (or read from) the URL, so Back, Forward or a reload fill the box without undoing typing.
	let text = $state('');
	let pushed: string | null = null;
	let searchTimer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		const fromUrl = params.filter.text ?? '';
		if (fromUrl !== pushed) {
			pushed = fromUrl;
			text = fromUrl;
		}
	});
	$effect(() => () => clearTimeout(searchTimer));

	function search() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => {
			pushed = text.trim();
			apply({ ...params.filter, text: pushed || undefined });
		}, SEARCH_DELAY_MS);
	}

	/** The reservation's modal, over this list with its filter and sort. */
	function rowHref(row: ReservationRoomRow) {
		return resolve(`/p/${propertyId}/reservations/${row.reservationId}${page.url.search}`);
	}

	function prefetch(row: ReservationRoomRow) {
		void client.prefetchQuery({
			queryKey: reservationKey(row.reservationId),
			queryFn: ({ signal }) => fetchReservation(propertyId, row.reservationId, signal)
		});
	}

	async function keydown(event: KeyboardEvent, index: number) {
		const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
		if (step === 0 || !scroller) return;
		event.preventDefault();
		const next = Math.max(0, Math.min(rows.length - 1, index + step));
		active = next;
		scroller.scrollTop = revealRow(
			next,
			scroller.scrollTop,
			scroller.clientHeight - ROW_HEIGHT,
			ROW_HEIGHT
		);
		await tick();
		const link = scroller.querySelector<HTMLElement>(`a[data-row="${next}"]`);
		link?.focus({ preventScroll: true });
		// The row is now in the scroller's view; this brings that part of the scroller into the window.
		link?.closest('[role="row"]')?.scrollIntoView({ block: 'nearest' });
	}

	// After the modal closes (and SvelteKit's own focus reset), focus returns to the row that opened it: the
	// active row when it is that reservation's (a reservation with several rooms has several rows), else its
	// first loaded row.
	afterNavigate(({ from, to }) => {
		const closed = from?.params?.id;
		if (!closed || to?.route.id !== '/(app)/p/[property]/reservations/(list)') return;
		const index =
			rows[active]?.reservationId === closed
				? active
				: rows.findIndex((row) => row.reservationId === closed);
		scroller?.querySelector<HTMLElement>(`a[data-row="${index}"]`)?.focus({ preventScroll: true });
	});

	function ariaSort(field: ReservationSortField | undefined) {
		if (!field) return undefined;
		if (params.sort.field !== field) return 'none';
		return params.sort.direction === 'ASC' ? 'ascending' : 'descending';
	}
</script>

<div class="title">
	<h1>Reservations</h1>
	{#if list.data}
		<p role="status">{total} reserved {total === 1 ? 'room' : 'rooms'}</p>
	{/if}
	<span class="spacer"></span>
	{#if manage}
		<a
			class="button"
			href={resolve('/(app)/p/[property]/reservations/new', { property: propertyId })}
			>New reservation</a
		>
	{/if}
</div>

<form class="inline-form" role="search" aria-label="Filters" onsubmit={(e) => e.preventDefault()}>
	<label>
		Search
		<input
			type="search"
			placeholder="Confirmation # or guest"
			maxlength="100"
			bind:value={text}
			oninput={search}
		/>
	</label>
	<label>
		Arrival from
		<input
			type="date"
			value={params.filter.arrivalFrom ?? ''}
			onchange={(event) =>
				apply({ ...params.filter, arrivalFrom: event.currentTarget.value || undefined })}
		/>
	</label>
	<label>
		Through
		<input
			type="date"
			value={params.filter.arrivalTo ?? ''}
			onchange={(event) =>
				apply({ ...params.filter, arrivalTo: event.currentTarget.value || undefined })}
		/>
	</label>
	<fieldset>
		<legend>Status</legend>
		{#each STATUSES as status (status)}
			<label class="check">
				<input
					type="checkbox"
					checked={params.filter.statuses?.includes(status) ?? true}
					onchange={(event) =>
						apply({
							...params.filter,
							statuses: toggleChoice(
								STATUSES,
								params.filter.statuses,
								status,
								event.currentTarget.checked
							)
						})}
				/>
				{statusLabel(status)}
			</label>
		{/each}
	</fieldset>
	<fieldset>
		<legend>Source</legend>
		{#each SOURCES as source (source)}
			<label class="check">
				<input
					type="checkbox"
					checked={params.filter.sources?.includes(source) ?? true}
					onchange={(event) =>
						apply({
							...params.filter,
							sources: toggleChoice(
								SOURCES,
								params.filter.sources,
								source,
								event.currentTarget.checked
							)
						})}
				/>
				{sourceLabel(source)}
			</label>
		{/each}
	</fieldset>
</form>

{#if list.isError && !list.data}
	<p class="error" role="alert">{errorMessage(list.error)}</p>
	<button onclick={() => list.refetch()}>Retry</button>
{:else if list.data && rows.length === 0}
	<p>{filtered ? 'No reservations match these filters.' : 'No reservations yet.'}</p>
{:else if list.data}
	<div
		class="scroller"
		role="table"
		aria-label="Reservations"
		aria-rowcount={total + 1}
		bind:this={scroller}
		bind:clientHeight={height}
		onscroll={() => (scrollTop = scroller?.scrollTop ?? 0)}
		style:--row="{ROW_HEIGHT}px"
	>
		<div class="header" role="rowgroup">
			<div class="cells" role="row" aria-rowindex={1}>
				{#each COLUMNS as column (column.label)}
					<div role="columnheader" aria-sort={ariaSort(column.sort)}>
						{#if column.sort}
							<button type="button" class="sort" onclick={() => sortBy(column.sort!)}>
								{column.label}
								{#if params.sort.field === column.sort}
									<span aria-hidden="true">{params.sort.direction === 'ASC' ? '▲' : '▼'}</span>
								{/if}
							</button>
						{:else}
							{column.label}
						{/if}
					</div>
				{/each}
			</div>
		</div>
		<div class="canvas" role="rowgroup" style:height="{rows.length * ROW_HEIGHT}px">
			{#each rendered as index (rows[index].id)}
				{@const row = rows[index]}
				<div
					class="row cells"
					role="row"
					aria-rowindex={index + 2}
					style:transform="translateY({index * ROW_HEIGHT}px)"
				>
					<div role="cell">
						<a
							href={rowHref(row)}
							data-row={index}
							data-sveltekit-noscroll
							tabindex={index === focusRow ? 0 : -1}
							onfocus={() => {
								active = index;
								prefetch(row);
							}}
							onpointerenter={() => prefetch(row)}
							onkeydown={(event) => keydown(event, index)}>{row.confirmationNo}</a
						>
					</div>
					<div role="cell">{row.guestName}</div>
					<div role="cell">{row.arrival}</div>
					<div role="cell">{row.departure}</div>
					<div role="cell" class="number">{row.nights}</div>
					<div role="cell">
						{row.roomTypeCode} ·
						{#if row.roomNumber}{row.roomNumber}{:else}<span class="hint">unassigned</span>{/if}
					</div>
					<div role="cell">{statusLabel(row.status)}</div>
					<div role="cell">{sourceLabel(row.source)}</div>
					<div role="cell" class="number">
						{row.currency}
						{formatMoney(row.total, row.currency)}
					</div>
					<div role="cell">{row.accountName ?? ''}</div>
				</div>
			{/each}
		</div>
	</div>
	{#if list.isFetchNextPageError}
		<p class="error" role="alert">{errorMessage(list.error)}</p>
		<button onclick={() => list.fetchNextPage()}>Retry</button>
	{:else if list.isFetchingNextPage}
		<p class="hint">Loading more…</p>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

{@render children()}

<style>
	.title {
		display: flex;
		align-items: baseline;
		gap: var(--space);
	}
	.spacer {
		flex: 1;
	}
	.button {
		padding: 0.45rem 0.6rem;
		border-radius: var(--radius);
		background: var(--accent);
		color: #fff;
		text-decoration: none;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.75rem;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.scroller {
		overflow: auto;
		max-height: 70vh;
		margin: var(--space) 0;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.cells {
		display: grid;
		grid-template-columns:
			8rem minmax(10rem, 2fr) 7rem 7rem 4rem minmax(8rem, 1fr) 7rem 8rem 9rem
			minmax(8rem, 1fr);
		min-width: 78rem;
		height: var(--row);
		border-bottom: 1px solid var(--border);
	}
	.cells > div {
		display: flex;
		align-items: center;
		padding: 0 0.6rem;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		gap: 0.25rem;
	}
	.header {
		position: sticky;
		top: 0;
		z-index: 1;
		background: var(--surface);
		font-weight: 600;
	}
	.sort {
		padding: 0;
		border: none;
		background: none;
		color: inherit;
		font-weight: inherit;
	}
	.canvas {
		position: relative;
		min-width: 78rem;
	}
	.canvas .row {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
	}
	.canvas .row:hover,
	.canvas .row:focus-within {
		background: var(--surface);
	}
	.canvas .row:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
	/* The confirmation link covers its whole row, so the row is the click (and hover) target. */
	.canvas a::after {
		content: '';
		position: absolute;
		inset: 0;
	}
	.canvas a:focus-visible {
		outline: none;
	}
	.number {
		justify-content: flex-end;
	}
</style>
