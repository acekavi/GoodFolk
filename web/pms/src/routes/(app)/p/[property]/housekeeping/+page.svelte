<!--
	The housekeeping board: one tile per active room, coloured by condition, with filters (kept in the URL),
	multi-select for bulk condition and assignment, Copy yesterday, and the open issues. Supervisor actions need
	`housekeepingSupervise`; the server enforces it. Tiles are one button each and read from the three cached
	lists, so a tile makes no query of its own. The issue badge counts the open issues list (`issues:`), the rest
	of the tile comes from the board (`hk:`), so either event refreshes what it feeds. Tiles have a fixed size and
	only the rows in view (plus overscan) are in the DOM; the grid holds the full height, and one tile at a time
	takes Tab (a roving tab stop) and the arrow, Home, End and Page keys. For everyone else the same markup is a
	read-only table whose scroller takes Tab itself, so the keyboard can scroll it.
-->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onMount, tick, untrack } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys } from '$lib/api/rest';
	import { revealRow, visibleRows } from '$lib/grid';
	import BlockDialog from '$lib/components/BlockDialog.svelte';
	import {
		assignRooms,
		conditionLabel,
		CONDITIONS,
		copyYesterday,
		fetchBoard,
		fetchHousekeepers,
		fetchOpenIssues,
		filterRooms,
		filtersFromSearchParams,
		filtersToSearchParams,
		hkKey,
		initials,
		issueCounts,
		issuesKey,
		moveTile,
		nextConditions,
		NO_FLOOR,
		OCCUPANCIES,
		occupancyLabel,
		rangeBetween,
		reportIssue,
		resolveIssue,
		setConditions,
		type BoardFilters,
		type BoardRoom,
		type Condition,
		type Occupancy,
		type RoomIssue,
		type Severity
	} from '$lib/housekeeping';
	import { Pending } from '$lib/pending.svelte';
	import { fetchRooms, fetchRoomTypes, roomsKey, roomTypesKey } from '$lib/rooms';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';

	const OCCUPANCY_GLYPH: Record<Occupancy, string> = {
		VACANT: '○',
		STAY_OVER: '●',
		DUE_OUT: '↗',
		DUE_IN: '↘',
		TURNOVER: '⇄'
	};
	const SEVERITIES: Severity[] = ['LOW', 'MEDIUM', 'HIGH'];

	// Tile geometry, in rems so that it follows the user's text size: a tile is `TILE` high and at least
	// `TILE_MIN` wide, with `GAP` between tiles, and the rows have a fixed pitch for the scroll maths.
	const TILE = 4.3;
	const TILE_MIN = 7.5;
	const GAP = 0.4;
	const OVERSCAN = 1;
	const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
	const pitch = (TILE + GAP) * rem;

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const pending = new Pending();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const allowed = $derived(!!me.data && can(me.data, 'housekeepingView', propertyId));
	const supervise = $derived(!!me.data && can(me.data, 'housekeepingSupervise', propertyId));
	const mayBlock = $derived(!!me.data && can(me.data, 'blockRooms', propertyId));
	const mayReport = $derived(!!me.data && can(me.data, 'roomIssuesReport', propertyId));
	const board = createQuery(() => ({
		queryKey: hkKey(propertyId),
		queryFn: ({ signal }) => fetchBoard(propertyId, signal),
		enabled: allowed
	}));
	const issues = createQuery(() => ({
		queryKey: issuesKey(propertyId),
		queryFn: ({ signal }) => fetchOpenIssues(propertyId, signal),
		enabled: allowed
	}));
	// Under the `hk:` key too, so it is refetched with the board; the list only changes with roles.
	const housekeepers = createQuery(() => ({
		queryKey: [...hkKey(propertyId), 'housekeepers'],
		queryFn: ({ signal }) => fetchHousekeepers(propertyId, signal),
		enabled: allowed
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal),
		enabled: allowed
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal),
		enabled: allowed
	}));

	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	const filters = $derived(filtersFromSearchParams(page.url.searchParams));
	const all = $derived(board.data ?? []);
	const shown = $derived(filterRooms(all, filters));
	const counts = $derived(issueCounts(issues.data ?? []));
	const numberOf = $derived(new Map(all.map((room) => [room.roomId, room.number])));
	const floors = $derived(
		[...new Set(all.map((room) => room.floor ?? NO_FLOOR))].sort((a, b) =>
			a === NO_FLOOR ? 1 : b === NO_FLOOR ? -1 : a.localeCompare(b, undefined, { numeric: true })
		)
	);
	const sections = $derived(rooms.data?.sections ?? []);

	// The issues, with the report form's option for every room, are below the board: they are made once the
	// board has painted, so they do not add to the time before it shows.
	let belowFold = $state(false);
	onMount(() => {
		const list = () => (belowFold = true);
		if ('requestIdleCallback' in window) {
			const id = requestIdleCallback(list, { timeout: 1000 });
			return () => cancelIdleCallback(id);
		}
		const id = setTimeout(list, 100);
		return () => clearTimeout(id);
	});

	let grid = $state<HTMLDivElement>();
	let scrollTop = $state(0);
	let width = $state(0);
	let height = $state(0);
	// The tile that takes Tab, by room id so that a refetch or a filter does not move it, kept rendered
	// wherever the board is scrolled.
	let activeId = $state<string>();
	const columns = $derived(Math.max(1, Math.floor((width / rem + GAP) / (TILE_MIN + GAP))));
	const rowCount = $derived(Math.ceil(shown.length / columns));
	const active = $derived(
		Math.max(
			0,
			shown.findIndex((room) => room.roomId === activeId)
		)
	);
	const range = $derived(visibleRows(scrollTop, height, pitch, rowCount, OVERSCAN));
	const rendered = $derived.by(() => {
		const inView = Array.from({ length: range.end - range.start }, (_, i) => range.start + i);
		const row = Math.floor(active / columns);
		return shown.length > 0 && (row < range.start || row >= range.end) ? [row, ...inView] : inView;
	});

	// A new filter is a new list: back to its top.
	$effect(() => {
		void filtersToSearchParams(filters).toString();
		untrack(() => {
			scrollTop = 0;
			if (grid) grid.scrollTop = 0;
		});
	});

	async function keydown(event: KeyboardEvent, index: number) {
		if (!grid || event.altKey || event.metaKey) return;
		const next = moveTile(
			index,
			event.key,
			event.ctrlKey,
			columns,
			shown.length,
			Math.max(1, Math.floor(height / pitch) - 1)
		);
		if (next === null) return;
		event.preventDefault();
		activeId = shown[next].roomId;
		grid.scrollTop = revealRow(
			Math.floor(next / columns),
			grid.scrollTop,
			height - GAP * rem,
			pitch
		);
		scrollTop = grid.scrollTop;
		await tick();
		grid.querySelector<HTMLElement>(`[data-index="${next}"]`)?.focus({ preventScroll: true });
	}

	const selected = new SvelteSet<string>();
	let anchor: string | undefined;
	let status = $state('');
	let problem = $state('');
	let blockFor = $state<RoomIssue>();
	let blocking = $state(false);
	let report = $state({ roomId: '', severity: 'MEDIUM' as Severity, description: '' });
	const reportKeys = formKeys();

	// A selection never holds a room the filters or a refetch took off the screen.
	$effect(() => {
		const visible = new Set(shown.map((room) => room.roomId));
		for (const id of selected) if (!visible.has(id)) selected.delete(id);
	});

	function filterBy(next: Partial<BoardFilters>) {
		const search = filtersToSearchParams({ ...filters, ...next }).toString();
		void goto(resolve(`/p/${propertyId}/housekeeping${search ? `?${search}` : ''}`), {
			replaceState: true,
			keepFocus: true,
			noScroll: true
		});
	}

	function pick(roomId: string, shift: boolean) {
		if (!supervise) return;
		activeId = roomId;
		if (shift) {
			for (const id of rangeBetween(
				shown.map((room) => room.roomId),
				anchor,
				roomId
			))
				selected.add(id);
		} else if (!selected.delete(roomId)) {
			selected.add(roomId);
		}
		anchor = roomId;
	}

	const tileName = (room: BoardRoom) =>
		[
			room.number,
			conditionLabel(room.condition),
			occupancyLabel(room.occupancy),
			room.assignee ? `cleaned by ${room.assignee.displayName}` : '',
			counts.get(room.roomId) ? `${counts.get(room.roomId)} open issues` : '',
			room.blockKind ? (room.blockKind === 'OUT_OF_ORDER' ? 'out of order' : 'out of service') : ''
		]
			.filter(Boolean)
			.join(', ');

	/** Runs a board command, shows its failure, and refreshes the board and issues (events do the same). */
	async function act(key: string, command: () => Promise<string | void>) {
		status = '';
		problem = '';
		try {
			status = (await pending.run(key, command)) ?? '';
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await Promise.all([
				client.invalidateQueries({ queryKey: hkKey(propertyId) }),
				client.invalidateQueries({ queryKey: issuesKey(propertyId) })
			]);
		}
	}

	function bulkCondition(value: string) {
		if (!value) return;
		const ids = [...selected];
		const condition = value as Condition;
		void act('bulk', async () => {
			await setConditions(propertyId, ids, condition);
			selected.clear();
			return `${ids.length} rooms set to ${conditionLabel(condition).toLowerCase()}`;
		});
	}

	function bulkAssign(value: string) {
		if (!value) return;
		const ids = [...selected];
		void act('bulk', async () => {
			await assignRooms(propertyId, ids, value === 'none' ? null : value);
			selected.clear();
			return value === 'none' ? `${ids.length} rooms unassigned` : `${ids.length} rooms assigned`;
		});
	}

	function copy() {
		void act('copy', async () => {
			const copied = await copyYesterday(propertyId);
			return `Copied ${copied} ${copied === 1 ? 'assignment' : 'assignments'} from yesterday`;
		});
	}

	function submitReport(event: SubmitEvent) {
		event.preventDefault();
		const { roomId, severity, description } = report;
		void act('report', async () => {
			try {
				await reportIssue(
					propertyId,
					roomId,
					severity,
					description.trim(),
					reportKeys.keyFor(report)
				);
			} catch (err) {
				reportKeys.failed(err);
				throw err;
			}
			reportKeys.reset();
			report = { roomId: '', severity: 'MEDIUM', description: '' };
			return 'Issue reported';
		});
	}

	function resolveOne(issue: RoomIssue) {
		void act(issue.id, async () => {
			await resolveIssue(propertyId, issue.id, issue.version);
			return `Issue in room ${numberOf.get(issue.roomId) ?? ''} resolved`;
		});
	}

	function blockRoom(issue: RoomIssue) {
		blockFor = issue;
		blocking = true;
	}

	// The room is blocked by the dialog, which only invalidates the inventory; the board shows the marker.
	$effect(() => {
		if (!blocking && blockFor) {
			blockFor = undefined;
			void client.invalidateQueries({ queryKey: hkKey(propertyId) });
		}
	});
</script>

{#snippet tileBody(room: BoardRoom)}
	{@const issueCount = counts.get(room.roomId)}
	<span class="number">{room.number}</span>
	<span class="condition" aria-hidden="true">{conditionLabel(room.condition)}</span>
	<span class="occupancy" aria-hidden="true"
		>{OCCUPANCY_GLYPH[room.occupancy]} {occupancyLabel(room.occupancy)}</span
	>
	<!-- Most tiles carry none of these three; one block decides for all of them. -->
	{#if room.assignee || issueCount || room.blockKind}
		{#if room.assignee}<span class="who" aria-hidden="true"
				>{initials(room.assignee.displayName)}</span
			>{/if}
		{#if issueCount}<span class="badge" aria-hidden="true">{issueCount}</span>{/if}
		{#if room.blockKind}<span class="block" aria-hidden="true"
				>{room.blockKind === 'OUT_OF_ORDER' ? 'OOO' : 'OOS'}</span
			>{/if}
	{/if}
{/snippet}

<div class="title">
	<h1>Housekeeping</h1>
	{#if businessDate}<span class="hint">{businessDate}</span>{/if}
	<span class="spacer"></span>
	{#if supervise}
		<button type="button" class="secondary" disabled={pending.has('copy')} onclick={copy}
			>Copy yesterday</button
		>
	{/if}
</div>

{#if me.data && !allowed}
	<p class="error" role="alert">You do not have access to housekeeping at this property.</p>
{:else if board.error || issues.error || properties.error}
	<p class="error" role="alert">
		{errorMessage(board.error ?? issues.error ?? properties.error)}
	</p>
{:else if !board.data || !issues.data}
	<p>Loading…</p>
{:else}
	<form class="filters" aria-label="Filters" onsubmit={(event) => event.preventDefault()}>
		<label>
			Floor
			<select value={filters.floor} onchange={(e) => filterBy({ floor: e.currentTarget.value })}>
				<option value="">All</option>
				{#each floors as floor (floor)}
					<option value={floor}>{floor === NO_FLOOR ? 'No floor' : floor}</option>
				{/each}
			</select>
		</label>
		<label>
			Section
			<select
				value={filters.section}
				onchange={(e) => filterBy({ section: e.currentTarget.value })}
			>
				<option value="">All</option>
				{#each sections as section (section.id)}
					<option value={section.id}>{section.name}</option>
				{/each}
			</select>
		</label>
		<label>
			Housekeeper
			<select
				value={filters.housekeeper}
				onchange={(e) => filterBy({ housekeeper: e.currentTarget.value })}
			>
				<option value="">All</option>
				<option value="none">Unassigned</option>
				{#each housekeepers.data ?? [] as person (person.userId)}
					<option value={person.userId}>{person.displayName}</option>
				{/each}
			</select>
		</label>
		<label>
			Condition
			<select
				value={filters.condition}
				onchange={(e) => filterBy({ condition: e.currentTarget.value as Condition | '' })}
			>
				<option value="">All</option>
				{#each CONDITIONS as condition (condition)}
					<option value={condition}>{conditionLabel(condition)}</option>
				{/each}
			</select>
		</label>
		<label>
			Occupancy
			<select
				value={filters.occupancy}
				onchange={(e) => filterBy({ occupancy: e.currentTarget.value as Occupancy | '' })}
			>
				<option value="">All</option>
				{#each OCCUPANCIES as occupancy (occupancy)}
					<option value={occupancy}>{occupancyLabel(occupancy)}</option>
				{/each}
			</select>
		</label>
		<span class="hint" role="status">{shown.length} of {all.length} rooms</span>
	</form>

	{#if supervise}
		<div class="bulk" role="toolbar" aria-label="Selected rooms">
			<span role="status">{selected.size} selected</span>
			<button
				type="button"
				class="secondary"
				disabled={shown.length === 0}
				onclick={() => {
					for (const room of shown) selected.add(room.roomId);
				}}>Select all shown</button
			>
			<button
				type="button"
				class="secondary"
				disabled={selected.size === 0}
				onclick={() => selected.clear()}>Clear</button
			>
			<select
				aria-label="Set condition…"
				disabled={selected.size === 0 || pending.has('bulk')}
				value=""
				onchange={(e) => {
					bulkCondition(e.currentTarget.value);
					e.currentTarget.value = '';
				}}
			>
				<option value="" disabled>Set condition…</option>
				{#each nextConditions('supervisor') as condition (condition)}
					<option value={condition}>{conditionLabel(condition)}</option>
				{/each}
			</select>
			<select
				aria-label="Assign to…"
				disabled={selected.size === 0 || pending.has('bulk')}
				value=""
				onchange={(e) => {
					bulkAssign(e.currentTarget.value);
					e.currentTarget.value = '';
				}}
			>
				<option value="" disabled>Assign to…</option>
				<option value="none">Unassign</option>
				{#each housekeepers.data ?? [] as person (person.userId)}
					<option value={person.userId}>{person.displayName}</option>
				{/each}
			</select>
		</div>
	{/if}

	{#if status}<p role="status">{status}</p>{/if}
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}

	{#if all.length === 0}
		<p>No active rooms yet.</p>
	{:else if shown.length === 0}
		<p>No rooms match the filters.</p>
	{:else}
		<!-- A scrollable region must take focus for the keyboard to scroll it. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			class="grid"
			role={supervise ? 'grid' : 'table'}
			tabindex={supervise ? undefined : 0}
			aria-label="Rooms"
			aria-rowcount={rowCount}
			aria-colcount={columns}
			bind:this={grid}
			bind:clientWidth={width}
			bind:clientHeight={height}
			onscroll={() => (scrollTop = grid?.scrollTop ?? 0)}
			style:--columns={columns}
			style:--tile="{TILE}rem"
			style:--gap="{GAP}rem"
		>
			<!-- Nothing is drawn until the grid has a width: its first measure comes before the first paint. -->
			<div class="canvas" role="rowgroup" style:height="{rowCount * pitch - GAP * rem}px">
				{#each width > 0 ? rendered : [] as row (row)}
					<div class="row" role="row" aria-rowindex={row + 1} style:top="{row * pitch}px">
						{#each shown.slice(row * columns, (row + 1) * columns) as room, column (room.roomId)}
							{@const index = row * columns + column}
							<div role={supervise ? 'gridcell' : 'cell'} aria-colindex={column + 1}>
								<!-- Only a supervisor can act on a tile; for everyone else it is a plain element with the same label. -->
								{#if supervise}
									<button
										type="button"
										class="tile"
										data-index={index}
										data-condition={room.condition}
										aria-label={tileName(room)}
										aria-pressed={selected.has(room.roomId)}
										tabindex={index === active ? 0 : -1}
										onclick={(event) => pick(room.roomId, event.shiftKey)}
										onkeydown={(event) => keydown(event, index)}
									>
										{@render tileBody(room)}
									</button>
								{:else}
									<div
										class="tile"
										role="img"
										data-condition={room.condition}
										aria-label={tileName(room)}
									>
										{@render tileBody(room)}
									</div>
								{/if}
							</div>
						{/each}
					</div>
				{/each}
			</div>
		</div>
	{/if}

	{#if belowFold}
		<section class="issues" aria-labelledby="issues-title">
			<h2 id="issues-title">Issues</h2>
			{#if mayReport}
				<form class="inline-form" aria-label="Report an issue" onsubmit={submitReport}>
					<label>
						Room
						<select required bind:value={report.roomId}>
							<option value="" disabled>Choose a room</option>
							{#each all as room (room.roomId)}
								<option value={room.roomId}>{room.number}</option>
							{/each}
						</select>
					</label>
					<label>
						Severity
						<select bind:value={report.severity}>
							{#each SEVERITIES as severity (severity)}
								<option value={severity}>{severity.toLowerCase()}</option>
							{/each}
						</select>
					</label>
					<label>
						Description
						<input required maxlength="500" bind:value={report.description} />
					</label>
					<button disabled={pending.has('report')}>Report issue</button>
				</form>
			{/if}
			{#if issues.data.length === 0}
				<p class="hint">No open issues.</p>
			{:else}
				<ul class="issue-list" aria-label="Open issues">
					{#each issues.data as issue (issue.id)}
						<li>
							<strong>Room {numberOf.get(issue.roomId) ?? '?'}</strong>
							<span class="severity" data-severity={issue.severity}
								>{issue.severity.toLowerCase()}</span
							>
							<span class="description">{issue.description}</span>
							{#if supervise}
								<button
									type="button"
									class="secondary"
									disabled={pending.has(issue.id)}
									onclick={() => resolveOne(issue)}>Resolve</button
								>
								{#if mayBlock && issue.severity === 'HIGH'}
									<button type="button" class="secondary" onclick={() => blockRoom(issue)}
										>Block room</button
									>
								{/if}
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	{/if}
{/if}

{#if mayBlock && rooms.data && roomTypes.data && businessDate}
	<BlockDialog
		{propertyId}
		{businessDate}
		rooms={rooms.data.rooms}
		roomTypes={roomTypes.data}
		reasons={rooms.data.blockReasons}
		from={businessDate}
		roomId={blockFor?.roomId}
		note={blockFor?.description.slice(0, 500)}
		bind:open={blocking}
	/>
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
	.filters,
	.bulk {
		display: flex;
		flex-wrap: wrap;
		align-items: end;
		gap: var(--space);
		margin-bottom: var(--space);
	}
	.bulk {
		align-items: center;
	}
	.grid {
		overflow: auto;
		max-height: 70vh;
	}
	.canvas {
		position: relative;
	}
	.row {
		position: absolute;
		left: 0;
		right: 0;
		height: var(--tile);
		display: grid;
		grid-template-columns: repeat(var(--columns), 1fr);
		gap: var(--gap);
	}
	[role='gridcell'] {
		min-width: 0;
	}
	.tile {
		width: 100%;
		height: var(--tile);
		box-sizing: border-box;
		align-content: start;
		--tint: var(--muted);
		position: relative;
		display: grid;
		gap: 0.1rem;
		justify-items: start;
		text-align: left;
		padding: 0.4rem 0.5rem;
		border: 1px solid var(--border);
		border-left: 0.35rem solid var(--tint);
		background: color-mix(in srgb, var(--tint) 14%, var(--bg));
		color: var(--text);
		contain: content;
	}
	.tile[data-condition='DIRTY'] {
		--tint: var(--danger);
	}
	.tile[data-condition='CLEANING'] {
		--tint: var(--stay-tentative);
	}
	.tile[data-condition='CLEAN'] {
		--tint: var(--stay-checked-in);
	}
	.tile[data-condition='INSPECTED'] {
		--tint: var(--accent);
	}
	.tile[aria-pressed='true'] {
		outline: 3px solid var(--accent);
		outline-offset: 1px;
	}
	.number {
		font-weight: 700;
		font-size: 1.1rem;
	}
	.condition,
	.occupancy {
		font-size: 0.8rem;
		color: var(--muted);
	}
	.who {
		position: absolute;
		right: 0.4rem;
		bottom: 0.4rem;
		font-size: 0.8rem;
		font-weight: 600;
	}
	.badge {
		position: absolute;
		right: 0.4rem;
		top: 0.4rem;
		min-width: 1.3rem;
		text-align: center;
		border-radius: 999px;
		background: var(--danger);
		color: var(--bg);
		font-size: 0.75rem;
		font-weight: 700;
	}
	.block {
		font-size: 0.7rem;
		font-weight: 700;
		border: 1px solid var(--text);
		border-radius: 3px;
		padding: 0 0.2rem;
	}
	.issues {
		margin-top: calc(var(--space) * 2);
	}
	.issue-list {
		list-style: none;
		padding: 0;
		display: grid;
		gap: 0.4rem;
	}
	.issue-list li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space);
		padding: 0.4rem 0;
		border-bottom: 1px solid var(--border);
	}
	.description {
		flex: 1;
	}
	.severity[data-severity='HIGH'] {
		color: var(--danger);
		font-weight: 600;
	}
</style>
