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

	Drag and drop (also `manage`): a confirmed stay is dragged by its body (to dates, another room or another
	type) or by either edge (to resize); a checked-in stay only by its departure edge. Pointer events with
	capture move one ghost element by `transform` alone, snapped to days, with nothing async or read from the
	cache in `pointermove`. A room-only drop saves at once through `assign`, with an Undo toast; dates and type
	ask first, with the old and new total. The cached tiles change on drop and roll back on a refusal.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQueries, createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { flushSync, tick, untrack } from 'svelte';
	import type { RoomStatus } from '$lib/api/gql/graphql';
	import { errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { addDays } from '$lib/inventory';
	import { formatMoney } from '$lib/rates';
	import {
		fetchAvailability,
		fetchReservation,
		findOffer,
		freeRoomsKey,
		modifyRoomBody,
		modifyRoomHasChanges,
		reservationKey,
		reservationListsKey,
		statusLabel,
		type ModifyRoomCurrent,
		type ModifyRoomDraft
	} from '$lib/reservations';
	import RoomAssign from './RoomAssign.svelte';
	import {
		assignStay,
		barDays,
		barsFor,
		barWidth,
		barX,
		clampOffset,
		dragDates,
		dragKind,
		dragPlan,
		dropTarget,
		fetchTapeTile,
		openingStart,
		pageKey,
		pruneTape,
		tapeKey,
		TILE_DAYS,
		tileStartFor,
		tilesFor,
		withStay,
		type DragHandle,
		type DragKind,
		type DragPlan,
		type RailRoom,
		type Span,
		type TapeBar,
		type TapeStay,
		type TapeTile
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
		/** A stay was opened, by click or Enter. */
		onopen: (reservationId: string) => void;
		/** PageUp (-1) or PageDown (1) was pressed. */
		onpage: (delta: -1 | 1) => void;
	}

	let {
		propertyId,
		rooms,
		start,
		span,
		businessDate,
		manage,
		onview,
		onopen,
		onstart,
		onpage
	}: Props = $props();

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
	/** Width of a bar's edge handles, which resize it. */
	const GRIP = 8;
	/** A pointer-down becomes a drag once it has moved this far (px); less is a click. */
	const DRAG_THRESHOLD = 4;
	const TOAST_MS = 8000;
	const SETTLE_MS = 200;
	const MENUS_DELAY_MS = 100;
	const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
	const MONTHS = [
		'Jan',
		'Feb',
		'Mar',
		'Apr',
		'May',
		'Jun',
		'Jul',
		'Aug',
		'Sep',
		'Oct',
		'Nov',
		'Dec'
	];

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
	// A bar's menu button is invisible until hovered, so a new room page paints its bars first and gets the
	// buttons a moment later: they are some third of the cost of a page change, and a reader paging through
	// the rooms never waits for (or pays for) the buttons of the pages passed over.
	let menusReady = $state(true);
	let menusTimer: ReturnType<typeof setTimeout> | undefined;
	$effect.pre(() => {
		void key;
		untrack(() => {
			menusReady = false;
			clearTimeout(menusTimer);
			menusTimer = setTimeout(() => (menusReady = true), MENUS_DELAY_MS);
		});
	});
	$effect(() => () => clearTimeout(menusTimer));
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

	// Spelled out like the weekdays: the first `Intl.DateTimeFormat` of a page costs some 20 ms.
	function monthName(date: string): string {
		return MONTHS[Number(date.slice(5, 7)) - 1];
	}

	// Scrolling: read once per frame; the URL hears of the settled start after a pause.
	function onscroll() {
		if (frame) return;
		frame = requestAnimationFrame(() => {
			frame = 0;
			if (!viewport) return;
			const left = viewport.scrollLeft;
			const day = left / dayWidth;
			// Whole pixels only: the browser rounds a scroll position set in days, which is no direction of travel.
			if (Math.abs(day - scrollDay) * dayWidth >= 1) direction = day > scrollDay ? 1 : -1;
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

	/** Shows `date` first now, dropping any scroll still settling so it cannot undo the jump. */
	export function jumpTo(date: string) {
		clearTimeout(settleTimer);
		settleTimer = undefined;
		focus = { row: focus.row, date: businessDate };
		scrollToDay(date);
		onstart(date);
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
	// `wanted` is a primitive, so a URL change that leaves the start alone (a modal opening over the chart,
	// a picker change) does not re-run the effect and snap back to a stale start.
	const wanted = $derived(start);
	$effect(() => {
		const target = wanted;
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
		// The menu is inside the chart; its keys are the menu's own (Enter chooses, Escape closes).
		if ((event.target as Element).closest('.menu')) return;
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
			jumpTo(openingStart(businessDate));
		} else if (
			(event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) &&
			focusBar &&
			canMove(focusBar)
		) {
			event.preventDefault();
			openMenu(focusBar);
		} else if (event.key === 'Enter' && focusBar?.kind === 'stay') {
			event.preventDefault();
			onopen(focusBar.reservationId);
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

	// Drag and drop.
	interface Grab {
		stay: TapeStay;
		el: HTMLElement;
		kind: DragKind;
		pointerId: number;
		startX: number;
		startY: number;
		/** The pointer's y within the lane when it was pressed. */
		laneY: number;
		x: number;
		y: number;
		width: number;
		nights: number;
		row: number;
		offset: number;
		roomId: string | null;
		dragging: boolean;
	}

	let ghost = $state<HTMLDivElement>();
	/** Set once when a drag begins, not per move. */
	let ghostLabel = $state('');
	let grab: Grab | undefined;
	/** A drag just ended, so the click that follows it must not open the stay. */
	let dragged = false;

	function grabBar(
		event: PointerEvent & { currentTarget: HTMLElement },
		bar: TapeBar,
		x: number,
		y: number,
		width: number
	) {
		dragged = false;
		if (!manage || bar.kind !== 'stay' || event.button !== 0 || dayWidth === 0) return;
		const handle = ((event.target as HTMLElement).dataset.handle ?? 'body') as DragHandle;
		const kind = dragKind(bar, handle);
		const row = rowOf.get(bar.roomId);
		if (!kind || row === undefined) return;
		const el = event.currentTarget;
		el.setPointerCapture(event.pointerId);
		grab = {
			stay: bar,
			el,
			kind,
			pointerId: event.pointerId,
			startX: event.clientX,
			startY: event.clientY,
			laneY: event.clientY - el.parentElement!.getBoundingClientRect().top,
			x,
			y,
			width,
			nights: barDays(bar.start, bar.end),
			row,
			offset: 0,
			roomId: bar.roomId,
			dragging: false
		};
	}

	// Only arithmetic and one style write per move: no queries, no state, no layout reads.
	function dragMove(event: PointerEvent) {
		const g = grab;
		if (!g || event.pointerId !== g.pointerId || !ghost) return;
		const dx = event.clientX - g.startX;
		const dy = event.clientY - g.startY;
		if (!g.dragging) {
			if (Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
			g.dragging = true;
			ghost.hidden = false;
			ghost.classList.add(g.kind);
			ghost.style.width = `${g.width}px`;
			ghostLabel = g.kind === 'move' ? g.stay.guestName : '';
			g.el.classList.add('dragging');
			viewport?.classList.add('is-dragging');
		}
		const target = dropTarget(dx, g.laneY + dy, dayWidth, ROW, roomIds);
		const days = clampOffset(g.kind, g.nights, target.dayOffset);
		g.offset = target.dayOffset;
		g.roomId = target.roomId;
		const row = target.roomId === null ? g.row : (rowOf.get(target.roomId) ?? g.row);
		const shift = days * dayWidth;
		ghost.style.transform =
			g.kind === 'move'
				? `translate(${g.x + shift}px, ${row * ROW + BAR_INSET}px)`
				: g.kind === 'resize-end'
					? `translate(${g.x}px, ${g.y}px) scaleX(${(g.width + shift) / g.width})`
					: `translate(${g.x + shift}px, ${g.y}px) scaleX(${(g.width - shift) / g.width})`;
	}

	function endGhost(g: Grab) {
		if (ghost) {
			ghost.hidden = true;
			ghost.classList.remove(g.kind);
		}
		g.el.classList.remove('dragging');
		viewport?.classList.remove('is-dragging');
	}

	function dragCancel() {
		const g = grab;
		grab = undefined;
		if (!g?.dragging) return;
		dragged = true;
		endGhost(g);
	}

	function dragEnd(event: PointerEvent) {
		const g = grab;
		if (!g || event.pointerId !== g.pointerId) return;
		grab = undefined;
		if (!g.dragging) return;
		dragged = true;
		endGhost(g);
		drop(g);
	}

	function drop(g: Grab) {
		const room = g.kind === 'move' && g.roomId ? rooms[rowOf.get(g.roomId) ?? -1] : undefined;
		const own = rooms[rowOf.get(g.stay.roomId) ?? -1];
		const target = room ?? own;
		if (!target) return;
		const plan = dragPlan(g.stay, g.kind, {
			roomId: target.id,
			roomTypeId: target.roomTypeId,
			dayOffset: g.offset
		});
		if (plan === 'refuse') return;
		if (plan === 'assign') {
			void reassign(g.stay, target, own);
			return;
		}
		const dates = dragDates(g.kind, g.stay, g.offset);
		confirming = { stay: g.stay, ...dates, room: target, plan, keepPrice: false };
	}

	// The cached tiles show a change as soon as it is made, and go back if the server refuses it.
	async function place(next: TapeStay): Promise<() => void> {
		await client.cancelQueries({ queryKey: ['tape', propertyId] });
		const snapshot = client.getQueriesData<TapeTile>({ queryKey: ['tape', propertyId] });
		apply(next);
		return () => {
			for (const [queryKey, data] of snapshot) client.setQueryData(queryKey, data);
		};
	}

	function apply(next: TapeStay) {
		for (const [queryKey, data] of client.getQueriesData<TapeTile>({
			queryKey: ['tape', propertyId]
		})) {
			if (!data) continue;
			const pageRooms = (queryKey[3] as string).split(',');
			client.setQueryData(queryKey, withStay(data, queryKey[2] as string, next, pageRooms));
		}
	}

	/** The server's events refetch these too; invalidating now shows the truth at once. */
	function refresh(reservationId: string) {
		return Promise.all([
			client.invalidateQueries({ queryKey: ['tape', propertyId] }),
			client.invalidateQueries({ queryKey: ['tape-unassigned', propertyId] }),
			client.invalidateQueries({ queryKey: reservationKey(reservationId) }),
			client.invalidateQueries({ queryKey: reservationListsKey(propertyId) }),
			client.invalidateQueries({ queryKey: freeRoomsKey(propertyId) })
		]);
	}

	let toast = $state<{ message: string; undo?: () => void }>();
	let toastTimer: ReturnType<typeof setTimeout> | undefined;

	/** Shows `message` for a few seconds; no message clears the toast. */
	function say(message?: string, undo?: () => void) {
		clearTimeout(toastTimer);
		toast = message ? { message, undo } : undefined;
		if (message) toastTimer = setTimeout(() => (toast = undefined), TOAST_MS);
	}
	$effect(() => () => clearTimeout(toastTimer));

	/**
	 * Puts the stay in room `to` with its version as If-Match. `from` (the room it leaves) makes the toast offer
	 * Undo, which assigns it back with the version this move produced; a 412 there shows the server's reason and
	 * is not retried.
	 */
	async function reassign(stay: TapeStay, to: RailRoom, from?: RailRoom) {
		const back = await place({ ...stay, roomId: to.id });
		try {
			const done = await assignStay(propertyId, stay.id, stay.version, to.id);
			const moved = { ...stay, roomId: to.id, version: done.version };
			apply(moved);
			if (from && from.id !== to.id) {
				say(`Moved to ${to.number}`, () => {
					say();
					void reassign(moved, from);
				});
			} else say(`Moved back to ${to.number}`);
		} catch (err) {
			back();
			say(errorMessage(err));
		} finally {
			await refresh(stay.reservationId);
		}
	}

	// Dates, resize and type: confirmed first, with the old and new total.
	let confirming = $state<{
		stay: TapeStay;
		start: string;
		end: string;
		room: RailRoom;
		plan: DragPlan;
		keepPrice: boolean;
	}>();
	let changeDialog = $state<HTMLDialogElement>();

	const typeCode = (roomTypeId: string) =>
		rooms.find((room) => room.roomTypeId === roomTypeId)?.typeCode ?? '?';
	const money = (amount: number, currency: string) =>
		`${currency} ${formatMoney(amount, currency)}`;

	const detail = createQuery(() => ({
		queryKey: reservationKey(confirming?.stay.reservationId ?? ''),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchReservation(propertyId, confirming!.stay.reservationId, signal),
		enabled: !!confirming
	}));
	/** The booked room being changed, once its reservation is fresh: its total and the plan it was sold on. */
	const priced = $derived(
		confirming && !detail.isFetching
			? detail.data?.rooms.find((room) => room.id === confirming!.stay.id)
			: undefined
	);
	// The same lookup as the reservation modal's Modify preview, so the two share a cache entry.
	const preview = createQuery(() => ({
		queryKey: [
			'reservationRoomPreview',
			propertyId,
			confirming?.start,
			confirming?.end,
			priced?.adults,
			priced?.children,
			priced?.primaryGuest.residency
		],
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			fetchAvailability(
				propertyId,
				confirming!.start,
				confirming!.end,
				priced!.adults,
				priced!.children,
				priced!.primaryGuest.residency,
				signal
			),
		enabled: !!confirming && !!priced
	}));
	const offer = $derived(
		confirming && priced && preview.data
			? findOffer(preview.data, confirming.room.roomTypeId, priced.ratePlan.id, priced.mealPlan)
			: undefined
	);

	$effect(() => {
		if (!changeDialog) return;
		if (confirming && !changeDialog.open) changeDialog.showModal();
		else if (!confirming && changeDialog.open) changeDialog.close();
	});

	async function confirmChange() {
		const change = confirming;
		const room = priced;
		if (!change || !room) return;
		confirming = undefined;
		const { stay } = change;
		const current: ModifyRoomCurrent = {
			checkIn: stay.start,
			checkOut: stay.end,
			roomTypeId: stay.roomTypeId,
			adults: room.adults,
			children: room.children
		};
		const typeChanged = change.room.roomTypeId !== stay.roomTypeId;
		const draft: ModifyRoomDraft = {
			...current,
			checkIn: change.start,
			checkOut: change.end,
			roomTypeId: change.room.roomTypeId,
			keepPrice: change.keepPrice && typeChanged,
			reprice: false
		};
		if (!modifyRoomHasChanges(current, draft)) return;
		const next: TapeStay = {
			...stay,
			start: change.start,
			end: change.end,
			roomTypeId: change.room.roomTypeId,
			roomId: change.plan === 'modify+assign' ? change.room.id : stay.roomId
		};
		const back = await place(next);
		try {
			const modified = unwrap(
				await rest.POST('/api/v1/properties/{property}/reservation-rooms/{room}/modify', {
					params: { path: { property: propertyId, room: stay.id }, header: ifMatch(stay.version) },
					body: modifyRoomBody(current, draft)
				})
			);
			let version = modified.version;
			// The server's own room: a type change picks one, or leaves the stay without.
			let roomId = modified.room_id ?? '';
			if (change.plan === 'modify+assign' && modified.room_id !== change.room.id) {
				try {
					version = (await assignStay(propertyId, stay.id, version, change.room.id)).version;
					roomId = change.room.id;
				} catch (err) {
					apply({ ...next, roomId, version });
					say(`Changed, but not moved to room ${change.room.number}: ${errorMessage(err)}`);
					return;
				}
			}
			apply({ ...next, roomId, version });
			say('Stay changed');
		} catch (err) {
			back();
			say(errorMessage(err));
		} finally {
			await refresh(stay.reservationId);
		}
	}
</script>

<svelte:window
	onpointerdown={(event) => {
		if (menuFor && !(event.target as Element).closest('.menu, .bar-menu')) menuFor = undefined;
	}}
	onkeydown={(event) => {
		if (event.key === 'Escape' && grab?.dragging) {
			event.preventDefault();
			dragCancel();
		}
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
							class:draggable={manage && dragKind(bar, 'body') !== null}
							tabindex="-1"
							draggable="false"
							data-room={rooms[rowOf.get(bar.roomId) ?? 0]?.number}
							data-sveltekit-noscroll
							aria-label={barLabel(bar)}
							style:transform="translate({x}px, {y}px)"
							style:width="{w}px"
							onpointerdown={(event) => grabBar(event, bar, x, y, w)}
							onpointermove={dragMove}
							onpointerup={dragEnd}
							onpointercancel={dragCancel}
							onclick={(event) => {
								// The click that ends a drag is not a request to open the stay.
								if (dragged) {
									dragged = false;
									event.preventDefault();
									return;
								}
								// A plain click opens the modal over the chart; a modified one keeps the link's own behaviour.
								if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey) return;
								event.preventDefault();
								onopen(bar.reservationId);
							}}
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
							{#if manage}
								{#if dragKind(bar, 'start')}
									<span class="grip start" data-handle="start" aria-hidden="true"></span>
								{/if}
								{#if dragKind(bar, 'end')}
									<span class="grip end" data-handle="end" aria-hidden="true"></span>
								{/if}
							{/if}
						</a>
						{#if menusReady && canMove(bar)}
							<button
								type="button"
								class="bar-menu"
								tabindex="-1"
								aria-label="Menu for {bar.guestName}"
								aria-haspopup="menu"
								aria-expanded={menuFor === bar.id}
								style:transform="translate({x + Math.max(w - BAR_MENU - GRIP, 0)}px, {y}px)"
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
				<div class="ghost" bind:this={ghost} hidden>{ghostLabel}</div>
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

<dialog
	bind:this={changeDialog}
	aria-labelledby="change-title"
	onclose={() => (confirming = undefined)}
>
	{#if confirming}
		{@const change = confirming}
		<h2 id="change-title">Change stay</h2>
		<p>
			{change.stay.guestName}: {change.stay.start} to {change.stay.end}
			→ {change.start} to {change.end}{#if change.room.roomTypeId !== change.stay.roomTypeId},
				{typeCode(change.stay.roomTypeId)} → {change.room.typeCode}{/if}
		</p>
		{#if change.plan === 'modify+assign'}
			<p>Then it moves to room {change.room.number}.</p>
		{/if}
		{#if change.room.roomTypeId !== change.stay.roomTypeId}
			<label class="check">
				<input type="checkbox" bind:checked={change.keepPrice} />
				Keep the booked price (upgrade)
			</label>
		{/if}
		{#if detail.isError || preview.isError}
			<p class="error" role="alert">{errorMessage(detail.error ?? preview.error)}</p>
		{:else if !priced || !preview.data}
			<p>Pricing…</p>
		{:else if offer}
			<p>
				Total <strong>{money(priced.total, priced.currency)}</strong> →
				<strong>{money(offer.total, offer.currency)}</strong>
				{#if change.keepPrice}
					<span class="hint"
						>(every night at the new stay's price; nights kept at their booked price will cost less)</span
					>
				{/if}
			</p>
		{:else}
			<p>
				Total <strong>{money(priced.total, priced.currency)}</strong>. No
				{typeCode(change.room.roomTypeId)} offer sells these nights on {priced.ratePlan.code}, so
				this change can't be made.
			</p>
		{/if}
		<div class="actions">
			<button type="button" disabled={!offer} onclick={confirmChange}>Confirm</button>
			<button type="button" class="secondary" onclick={() => (confirming = undefined)}
				>Cancel</button
			>
		</div>
	{/if}
</dialog>

{#if toast}
	<div class="toast" role={toast.undo ? 'status' : 'alert'}>
		<span>{toast.message}</span>
		{#if toast.undo}
			<button type="button" onclick={toast.undo}>Undo</button>
		{/if}
		<button type="button" class="secondary" aria-label="Dismiss" onclick={() => say()}>×</button>
	</div>
{/if}

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
	}
	.bar.draggable {
		cursor: grab;
		touch-action: none;
		user-select: none;
	}
	.bar:global(.dragging) {
		opacity: 0.4;
	}
	.viewport:global(.is-dragging) {
		cursor: grabbing;
		user-select: none;
	}
	.grip {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 8px;
		cursor: ew-resize;
	}
	.grip.start {
		left: 0;
	}
	.grip.end {
		right: 0;
	}
	.ghost {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 4;
		height: calc(var(--row) - 8px);
		padding: 0 0.4rem;
		overflow: hidden;
		border: 2px dashed var(--accent);
		border-radius: 4px;
		background: color-mix(in srgb, var(--accent) 25%, transparent);
		color: var(--text);
		white-space: nowrap;
		line-height: calc(var(--row) - 12px);
		pointer-events: none;
		transform-origin: 0 0;
		will-change: transform;
	}
	.toast {
		position: fixed;
		bottom: 1rem;
		left: 50%;
		z-index: 20;
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.5rem 0.75rem;
		transform: translateX(-50%);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
		box-shadow: 0 2px 8px rgb(0 0 0 / 0.3);
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
