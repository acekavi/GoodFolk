<!--
	The tape chart: rooms down the side, days across, a bar per stay or block. It is built to stay fast:

	- One scroll container. The room rail (`position: sticky; left`) and the date header (`position: sticky;
	  top`) stay put without scroll handlers.
	- The lane has no per-cell nodes. Day lines are one repeating gradient; weekends and the business date are
	  one shaded element per loaded tile.
	- A bar is one element placed with `transform`; only the bars of the tiles in view exist.
	- Scrolling is read once per animation frame. The tiles it reveals are fetched (one more in the direction
	  of travel), the tiles it leaves are cancelled, and `pruneTape` keeps the cache to twelve.
	- The canvas is a fixed span of days around the view; it is re-centred (the scroll position adjusted in the
	  same frame) when the view nears an edge, so any date can be reached.

	Keyboard: one focus stop on the chart. Arrow keys move the focused day and room, Enter opens the stay
	under the focus, PageUp and PageDown page the rooms, T returns to the business date. The focus is
	announced through a status region, not through nodes per cell.

	A confirmed stay's menu (a button that shows on hover or focus, the context menu, or the context-menu key
	on the focused bar) offers Move to room…, which reassigns it to a free room of its type, also one on
	another page. It needs `manage`.
-->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQueries, useQueryClient } from '@tanstack/svelte-query';
	import { flushSync, tick, untrack } from 'svelte';
	import type { RoomStatus } from '$lib/api/gql/graphql';
	import { errorMessage } from '$lib/api/problem';
	import { addDays } from '$lib/inventory';
	import { fetchReservation, reservationKey, statusLabel } from '$lib/reservations';
	import RoomAssign from './RoomAssign.svelte';
	import {
		barsFor,
		barWidth,
		barX,
		fetchTapeTile,
		openingStart,
		pageKey,
		pruneTape,
		tapeKey,
		TILE_DAYS,
		tileStartFor,
		tilesFor,
		type RailRoom,
		type Span,
		type TapeBar,
		type TapeStay
	} from '$lib/tape';

	interface Props {
		propertyId: string;
		/** The page of rooms shown, in rail order. */
		rooms: RailRoom[];
		/** The first day in view, from the URL; scrolled to whenever it differs from what is on screen. */
		start: string;
		span: Span;
		businessDate: string;
		/** Whether the user may change reservations; read-only otherwise. */
		manage: boolean;
		/** The first day in view changed, as scrolling moves it. */
		onview: (date: string) => void;
		/** The view's first day settled on `date`. */
		onstart: (date: string) => void;
		/** PageUp (-1) or PageDown (1) was pressed. */
		onpage: (delta: -1 | 1) => void;
	}

	let { propertyId, rooms, start, span, businessDate, manage, onview, onstart, onpage }: Props =
		$props();

	const RAIL = 112;
	const ROW = 44;
	const HEADER = 44;
	const BAR_INSET = 4;
	/** Days the canvas spans, centred on the view. */
	const CANVAS_DAYS = 52 * TILE_DAYS;
	/** The canvas is re-centred when the view comes this close to its edge. */
	const EDGE_DAYS = 3 * TILE_DAYS;
	const SECOND_LINE_MIN = 120;
	const BAR_MENU = 24;
	const SETTLE_MS = 200;
	const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

	const client = useQueryClient();

	let viewport = $state<HTMLDivElement>();
	let width = $state(0);
	/** The scroll position in days from the canvas origin, so a change of `dayWidth` does not move the view. */
	let scrollDay = $state(untrack(() => barX(start, centred(start), 1)));
	let direction = $state<-1 | 0 | 1>(0);
	let positioned = $state(false);
	let origin = $state(untrack(() => centred(start)));
	let focus = $state({ row: 0, date: untrack(() => businessDate) });
	let frame = 0;
	let settleTimer: ReturnType<typeof setTimeout> | undefined;

	const dayWidth = $derived(width > RAIL ? (width - RAIL) / span : 0);
	const dayAt = (date: string) => barX(date, origin, 1);
	const firstDay = $derived(positioned ? addDays(origin, Math.round(scrollDay)) : start);
	$effect(() => onview(firstDay));

	/** The canvas origin that puts `date` mid-canvas, on a tile boundary. */
	function centred(date: string): string {
		return tileStartFor(addDays(date, -CANVAS_DAYS / 2));
	}

	// The tiles in view plus one ahead; a string key so the list only changes when the set does.
	const tilesKey = $derived(tilesFor(firstDay, span + 1, 1, direction).join(','));
	const tiles = $derived(tilesKey.split(','));
	const roomIds = $derived(rooms.map((room) => room.id));
	const key = $derived(pageKey(roomIds));

	const tileQueries = createQueries(() => ({
		queries: tiles.map((tile) => ({
			queryKey: tapeKey(propertyId, tile, key),
			queryFn: ({ signal }: { signal: AbortSignal }) =>
				fetchTapeTile(propertyId, tile, roomIds, signal),
			staleTime: Infinity,
			enabled: roomIds.length > 0
		}))
	}));
	const failed = $derived(tileQueries.find((query) => query.isError));
	const loading = $derived(tileQueries.some((query) => query.isPending && query.isFetching));

	// Tiles that left the view stop loading; the cache keeps the twelve most recent.
	$effect(() => {
		const inView = new Set(tiles);
		const current = key;
		untrack(() => {
			void client.cancelQueries({
				queryKey: ['tape', propertyId],
				predicate: (query) =>
					query.queryKey[3] === current && !inView.has(query.queryKey[2] as string)
			});
			pruneTape(client);
		});
	});
	$effect(() => {
		void tileQueries.map((query) => query.dataUpdatedAt).join();
		untrack(() => pruneTape(client));
	});

	const rowOf = $derived(new Map(rooms.map((room, index) => [room.id, index])));
	const bars = $derived(
		barsFor(tileQueries.flatMap((query) => (query.data ? [query.data] : [])))
			.filter((bar) => rowOf.has(bar.roomId))
			.map((bar) => {
				const x = barX(bar.start, origin, dayWidth);
				const w = barWidth(bar.start, bar.end, dayWidth);
				const clipped = Math.max(0, -x);
				return { bar, x: x + clipped, w: w - clipped, y: rowOf.get(bar.roomId)! * ROW + BAR_INSET };
			})
			.filter((placed) => placed.w > 0)
	);

	function statusClass(status: string): string {
		return `stay-${status.toLowerCase().replaceAll('_', '-')}`;
	}

	function prefetch(reservationId: string) {
		void client.prefetchQuery({
			queryKey: reservationKey(reservationId),
			queryFn: ({ signal }) => fetchReservation(propertyId, reservationId, signal)
		});
	}

	function barLabel(bar: TapeBar): string {
		const room = rooms[rowOf.get(bar.roomId) ?? 0]?.number;
		return bar.kind === 'stay'
			? `${bar.guestName}, room ${room}, ${bar.start} to ${bar.end}, ${statusLabel(bar.status as RoomStatus)}`
			: `Blocked: ${bar.reason}, room ${room}, ${bar.start} to ${bar.end}`;
	}

	const headerDays = $derived(
		tiles.flatMap((tile) =>
			Array.from({ length: TILE_DAYS }, (_, index) => {
				const date = addDays(tile, index);
				return {
					date,
					weekday: WEEKDAYS[(index + 1) % 7],
					number: Number(date.slice(8)),
					month: date.slice(8) === '01' || date === firstDay ? monthName(date) : ''
				};
			})
		)
	);

	function monthName(date: string): string {
		return new Date(`${date}T00:00:00Z`).toLocaleDateString(undefined, {
			month: 'short',
			timeZone: 'UTC'
		});
	}

	// Scrolling: read once per frame; the URL hears of the settled start after a pause.
	function onscroll() {
		if (frame) return;
		frame = requestAnimationFrame(() => {
			frame = 0;
			if (!viewport) return;
			const left = viewport.scrollLeft;
			const day = left / dayWidth;
			if (day !== scrollDay) direction = day > scrollDay ? 1 : -1;
			scrollDay = day;
			recentre();
			clearTimeout(settleTimer);
			settleTimer = setTimeout(settle, SETTLE_MS);
		});
	}

	function settle() {
		settleTimer = undefined;
		if (firstDay !== start) onstart(firstDay);
	}

	/** Moves the canvas so `date` is mid-canvas, without moving what is on screen. */
	function recentreOn(date: string) {
		if (!viewport || dayWidth === 0) return;
		const next = centred(date);
		if (next === origin) return;
		const shift = barX(origin, next, 1) * dayWidth;
		origin = next;
		flushSync();
		viewport.scrollLeft += shift;
		scrollDay = viewport.scrollLeft / dayWidth;
	}

	function recentre() {
		const index = Math.round(scrollDay);
		if (index < EDGE_DAYS || index > CANVAS_DAYS - span - EDGE_DAYS) {
			recentreOn(addDays(origin, index));
		}
	}

	function scrollToDay(date: string) {
		if (!viewport) return;
		const index = dayAt(date);
		if (index < EDGE_DAYS || index > CANVAS_DAYS - span - EDGE_DAYS) recentreOn(date);
		viewport.scrollLeft = dayAt(date) * dayWidth;
		scrollDay = dayAt(date);
	}

	// A new span changes the day width: keep the same day first. After the canvas has taken its width, or
	// the browser clamps the scroll position.
	$effect(() => {
		if (dayWidth === 0) return;
		void tick().then(() =>
			untrack(() => {
				if (!viewport) return;
				scrollToDay(positioned ? addDays(origin, Math.round(scrollDay)) : start);
				positioned = true;
			})
		);
	});
	// Follow the URL's start (Today, Back) unless a scroll is still settling.
	$effect(() => {
		const target = start;
		untrack(() => {
			if (positioned && settleTimer === undefined && firstDay !== target) scrollToDay(target);
		});
	});
	$effect(() => () => {
		clearTimeout(settleTimer);
		cancelAnimationFrame(frame);
	});

	// Keyboard.
	const focusRoom = $derived(rooms[Math.min(focus.row, rooms.length - 1)]);
	const focusBar = $derived(
		focusRoom
			? bars.find(
					({ bar }) =>
						bar.roomId === focusRoom.id && bar.start <= focus.date && focus.date < bar.end
				)?.bar
			: undefined
	);
	const focusLabel = $derived(
		focusRoom ? `${focusRoom.number} ${focus.date}: ${focusBar ? barLabel(focusBar) : 'free'}` : ''
	);

	function moveFocus(rowDelta: number, dayDelta: number) {
		if (!viewport || rooms.length === 0) return;
		const row = Math.min(Math.max(focus.row + rowDelta, 0), rooms.length - 1);
		const index = Math.min(Math.max(dayAt(focus.date) + dayDelta, 0), CANVAS_DAYS - 1);
		const date = addDays(origin, index);
		focus = { row, date };
		const left = index * dayWidth;
		const visible = width - RAIL;
		if (left < viewport.scrollLeft) viewport.scrollLeft = left;
		else if (left + dayWidth > viewport.scrollLeft + visible) {
			viewport.scrollLeft = left + dayWidth - visible;
		}
	}

	function keydown(event: KeyboardEvent) {
		if (event.ctrlKey || event.metaKey || event.altKey) return;
		const moves: Record<string, [number, number]> = {
			ArrowLeft: [0, -1],
			ArrowRight: [0, 1],
			ArrowUp: [-1, 0],
			ArrowDown: [1, 0]
		};
		if (event.key in moves) {
			event.preventDefault();
			moveFocus(...moves[event.key]);
		} else if (event.key === 'PageUp' || event.key === 'PageDown') {
			event.preventDefault();
			onpage(event.key === 'PageUp' ? -1 : 1);
		} else if (event.key === 't' || event.key === 'T') {
			event.preventDefault();
			focus = { row: focus.row, date: businessDate };
			onstart(openingStart(businessDate));
		} else if (
			(event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) &&
			focusBar &&
			canMove(focusBar)
		) {
			event.preventDefault();
			openMenu(focusBar);
		} else if (event.key === 'Enter' && focusBar?.kind === 'stay') {
			event.preventDefault();
			// The reservation's modal over the reservations list, keeping the chart's view in the URL.
			void goto(
				resolve(`/p/${propertyId}/reservations/${focusBar.reservationId}${page.url.search}`),
				{ noScroll: true }
			);
		}
	}

	const todayIndex = (tile: string) => barX(businessDate, tile, 1);

	// A stay's menu and Move to room…. Only confirmed stays can be assigned.
	let menuFor = $state<string>();
	let movingId = $state<string>();
	let menuItem = $state<HTMLButtonElement>();
	let moveDialog = $state<HTMLDialogElement>();

	const canMove = (bar: TapeBar): bar is { kind: 'stay' } & TapeStay =>
		manage && bar.kind === 'stay' && bar.status === 'CONFIRMED';
	const menuPlaced = $derived(bars.find(({ bar }) => bar.kind === 'stay' && bar.id === menuFor));
	const moving = $derived(
		bars.find(({ bar }) => bar.kind === 'stay' && bar.id === movingId)?.bar as
			({ kind: 'stay' } & TapeStay) | undefined
	);

	function openMenu(bar: TapeBar) {
		if (canMove(bar)) menuFor = bar.id;
	}

	function closeMenu() {
		menuFor = undefined;
		viewport?.focus();
	}

	function startMove() {
		movingId = menuFor;
		menuFor = undefined;
	}

	// The menu's item takes the focus when the menu opens, so the keyboard can choose it.
	$effect(() => {
		if (menuPlaced) void tick().then(() => menuItem?.focus());
	});
	$effect(() => {
		if (!moveDialog) return;
		if (moving && !moveDialog.open) moveDialog.showModal();
		else if (!moving && moveDialog.open) moveDialog.close();
	});

	function menuKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') {
			event.preventDefault();
			event.stopPropagation();
			closeMenu();
		}
	}
</script>

<svelte:window
	onpointerdown={(event) => {
		if (menuFor && !(event.target as Element).closest('.menu, .bar-menu')) menuFor = undefined;
	}}
/>

{#if failed}
	<p class="error" role="alert">
		{errorMessage(failed.error)}
		<button type="button" class="secondary" onclick={() => failed?.refetch()}>Retry</button>
	</p>
{/if}
<!-- The chart is one focus stop driven from the keyboard, announced through its status region. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
	class="viewport"
	role="group"
	tabindex="0"
	aria-label="Tape chart"
	aria-busy={loading}
	data-start={firstDay}
	data-editable={manage}
	bind:this={viewport}
	bind:clientWidth={width}
	{onscroll}
	onkeydown={keydown}
>
	<div
		class="canvas"
		style:width="calc(var(--rail) + var(--day) * {CANVAS_DAYS})"
		style:--day="{dayWidth}px"
		style:--rail="{RAIL}px"
		style:--row="{ROW}px"
		style:--header="{HEADER}px"
	>
		<div class="header">
			<div class="corner"></div>
			<div class="days">
				{#each headerDays as day (day.date)}
					<div
						class="day"
						class:today={day.date === businessDate}
						class:weekend={day.weekday === 'Sat' || day.weekday === 'Sun'}
						style:transform="translateX({barX(day.date, origin, dayWidth)}px)"
					>
						<small>{day.month || day.weekday}</small>
						<span>{day.number}</span>
					</div>
				{/each}
			</div>
		</div>
		<div class="body" style:height="{rooms.length * ROW}px">
			<ol class="rail" aria-label="Rooms">
				{#each rooms as room (room.id)}
					<li><strong>{room.number}</strong> <span class="hint">{room.typeCode}</span></li>
				{/each}
			</ol>
			<div class="lane">
				{#each tiles as tile (tile)}
					{@const today = todayIndex(tile)}
					<div
						class="shade"
						class:has-today={today >= 0 && today < TILE_DAYS}
						style:transform="translateX({barX(tile, origin, dayWidth)}px)"
						style:--today-index={today}
					></div>
				{/each}
				{#each bars as { bar, x, w, y } (`${bar.kind}:${bar.id}`)}
					{#if bar.kind === 'stay'}
						<a
							class="bar {statusClass(bar.status)}"
							href={resolve(`/p/${propertyId}/reservations/${bar.reservationId}${page.url.search}`)}
							tabindex="-1"
							data-room={rooms[rowOf.get(bar.roomId) ?? 0]?.number}
							data-sveltekit-noscroll
							aria-label={barLabel(bar)}
							style:transform="translate({x}px, {y}px)"
							style:width="{w}px"
							onpointerenter={() => prefetch(bar.reservationId)}
							oncontextmenu={(event) => {
								if (!canMove(bar)) return;
								event.preventDefault();
								openMenu(bar);
							}}
						>
							<span class="name">{bar.guestName}</span>
							{#if bar.accountName && w > SECOND_LINE_MIN}
								<span class="account">{bar.accountName}</span>
							{/if}
						</a>
						{#if canMove(bar)}
							<button
								type="button"
								class="bar-menu"
								tabindex="-1"
								aria-label="Menu for {bar.guestName}"
								aria-haspopup="menu"
								aria-expanded={menuFor === bar.id}
								style:transform="translate({x + Math.max(w - BAR_MENU, 0)}px, {y}px)"
								onclick={() => openMenu(bar)}>⋯</button
							>
						{/if}
					{:else}
						<div
							class="bar block"
							data-room={rooms[rowOf.get(bar.roomId) ?? 0]?.number}
							title={barLabel(bar)}
							style:transform="translate({x}px, {y}px)"
							style:width="{w}px"
						>
							<span class="name">{bar.reason}</span>
						</div>
					{/if}
				{/each}
				{#if menuPlaced && menuPlaced.bar.kind === 'stay'}
					<div
						class="menu"
						role="menu"
						aria-label="Menu for {menuPlaced.bar.guestName}"
						tabindex="-1"
						style:transform="translate({menuPlaced.x}px, {menuPlaced.y + ROW - BAR_INSET}px)"
						onkeydown={menuKeydown}
					>
						<button type="button" role="menuitem" bind:this={menuItem} onclick={startMove}
							>Move to room…</button
						>
					</div>
				{/if}
				{#if focusRoom}
					<div
						class="marker"
						style:transform="translate({dayAt(focus.date) * dayWidth}px, {Math.min(
							focus.row,
							rooms.length - 1
						) * ROW}px)"
					></div>
				{/if}
			</div>
		</div>
	</div>
	<div class="visually-hidden" role="status">{focusLabel}</div>
</div>

<dialog bind:this={moveDialog} aria-labelledby="move-title" onclose={() => (movingId = undefined)}>
	{#if moving}
		<h2 id="move-title">Move to room</h2>
		<p>
			{moving.guestName}, {moving.start} to {moving.end}
		</p>
		<RoomAssign
			{propertyId}
			stay={moving}
			typeCode={rooms[rowOf.get(moving.roomId) ?? 0]?.typeCode ?? ''}
			action="Move"
			ondone={() => (movingId = undefined)}
			oncancel={() => (movingId = undefined)}
		/>
	{/if}
</dialog>

<style>
	.viewport {
		position: relative;
		overflow: auto;
		max-height: 75vh;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.viewport:focus-visible {
		outline: 2px solid var(--accent);
	}
	.canvas {
		position: relative;
	}
	.header {
		position: sticky;
		top: 0;
		z-index: 3;
		display: flex;
		height: var(--header);
		background: var(--surface);
		border-bottom: 1px solid var(--border);
	}
	.corner {
		position: sticky;
		left: 0;
		z-index: 4;
		flex: none;
		width: var(--rail);
		background: var(--surface);
		border-right: 1px solid var(--border);
	}
	.days {
		position: relative;
		flex: 1;
	}
	.day {
		position: absolute;
		top: 0;
		left: 0;
		display: grid;
		align-content: center;
		justify-items: center;
		width: var(--day);
		height: 100%;
		line-height: 1.1;
		border-left: 1px solid var(--border);
	}
	.day small {
		color: var(--muted);
	}
	.day.weekend small {
		color: var(--danger);
	}
	.day.today {
		color: var(--accent);
		font-weight: 600;
	}
	.body {
		display: flex;
	}
	.rail {
		position: sticky;
		left: 0;
		z-index: 2;
		flex: none;
		width: var(--rail);
		margin: 0;
		padding: 0;
		list-style: none;
		background: var(--surface);
		border-right: 1px solid var(--border);
	}
	.rail li {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		height: var(--row);
		padding: 0 0.6rem;
		border-bottom: 1px solid var(--border);
	}
	.lane {
		position: relative;
		flex: 1;
		/* A day line each `--day`, and a line under each row. */
		background-image:
			repeating-linear-gradient(to right, var(--border) 0 1px, transparent 1px var(--day)),
			linear-gradient(
				to bottom,
				transparent calc(var(--row) - 1px),
				var(--border) calc(var(--row) - 1px)
			);
		background-size:
			auto,
			100% var(--row);
	}
	/* A tile is 14 days from a Monday, so its weekends are days 5-6 and 12-13. */
	.shade {
		position: absolute;
		top: 0;
		left: 0;
		width: calc(var(--day) * 14);
		height: 100%;
		pointer-events: none;
		background-image: linear-gradient(
			to right,
			transparent calc(var(--day) * 5),
			var(--shade-weekend) calc(var(--day) * 5) calc(var(--day) * 7),
			transparent calc(var(--day) * 7) calc(var(--day) * 12),
			var(--shade-weekend) calc(var(--day) * 12)
		);
	}
	.shade.has-today {
		background-image:
			linear-gradient(
				to right,
				transparent calc(var(--day) * var(--today-index)),
				var(--shade-today) calc(var(--day) * var(--today-index))
					calc(var(--day) * (var(--today-index) + 1)),
				transparent calc(var(--day) * (var(--today-index) + 1))
			),
			linear-gradient(
				to right,
				transparent calc(var(--day) * 5),
				var(--shade-weekend) calc(var(--day) * 5) calc(var(--day) * 7),
				transparent calc(var(--day) * 7) calc(var(--day) * 12),
				var(--shade-weekend) calc(var(--day) * 12)
			);
	}
	.bar {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 1;
		display: grid;
		align-content: center;
		height: calc(var(--row) - 8px);
		padding: 0 0.4rem;
		overflow: hidden;
		border-radius: 4px;
		color: var(--stay-text);
		background: var(--stay-confirmed);
		text-decoration: none;
		line-height: 1.15;
		will-change: transform;
	}
	.bar .name,
	.bar .account {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.bar .account {
		font-size: 0.8em;
		opacity: 0.85;
	}
	.stay-tentative {
		background: var(--stay-tentative);
	}
	.stay-checked-in {
		background: var(--stay-checked-in);
	}
	.stay-checked-out {
		background: var(--stay-checked-out);
	}
	.bar.block {
		color: var(--text);
		border: 1px solid var(--muted);
		background: repeating-linear-gradient(45deg, var(--surface) 0 6px, var(--border) 6px 12px);
	}
	.bar-menu {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 2;
		width: 24px;
		height: calc(var(--row) - 8px);
		padding: 0;
		opacity: 0;
		border: 0;
		color: var(--stay-text);
		background: transparent;
	}
	.bar:hover + .bar-menu,
	.bar-menu:hover,
	.bar-menu:focus-visible,
	.bar-menu[aria-expanded='true'] {
		opacity: 1;
	}
	.menu {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 3;
		display: grid;
		min-width: 9rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		box-shadow: 0 2px 8px rgb(0 0 0 / 0.2);
	}
	.menu button {
		text-align: left;
		border: 0;
		background: transparent;
		color: var(--text);
	}
	.marker {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 1;
		display: none;
		width: var(--day);
		height: var(--row);
		outline: 2px solid var(--accent);
		outline-offset: -2px;
		pointer-events: none;
	}
	.viewport:focus-visible .marker {
		display: block;
	}
</style>
