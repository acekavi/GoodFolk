<!--
	A grid of rows by dates that renders only the columns in view (horizontal virtualization).
	One scroll container; the date header and the row labels stay put with `position: sticky`; the day
	lines are a CSS gradient, so there are no per-cell background nodes. The inventory calendar uses it
	now and the tape chart will later.

	Keyboard: arrow keys move between cells, Home and End go to the first and last date, Enter or Space
	activates the cell. The active cell is announced through `aria-activedescendant`.
-->
<script lang="ts" generics="Row extends { id: string; label: string }">
	import type { Snippet } from 'svelte';
	import { clampCell, moveFocus, revealColumn, visibleColumns, type Cell } from '$lib/grid';

	interface Props {
		/** Accessible name of the grid. */
		label: string;
		rows: Row[];
		/** Dates as `YYYY-MM-DD`, one column each. */
		columns: string[];
		/** Content of a cell. */
		cell: Snippet<[Row, string]>;
		/** Content of a column header. */
		header: Snippet<[string]>;
		/** Accessible name of a cell. */
		cellLabel: (row: Row, column: string) => string;
		/** The column that is active and scrolled into view first. */
		initialColumn?: number;
		onactivate?: (row: Row, column: string) => void;
		columnWidth?: number;
		rowHeight?: number;
		railWidth?: number;
		overscan?: number;
	}

	let {
		label,
		rows,
		columns,
		cell,
		header,
		cellLabel,
		initialColumn = 0,
		onactivate,
		columnWidth = 64,
		rowHeight = 44,
		railWidth = 160,
		overscan = 2
	}: Props = $props();

	const id = $props.id();
	let viewport = $state<HTMLDivElement>();
	let scrollLeft = $state(0);
	let width = $state(0);
	let chosen = $state<Cell>({ row: 0, column: 0 });
	// The chosen cell, kept inside the grid when rows or columns go away (a room type is retired), so
	// `aria-activedescendant` always names a rendered cell.
	const active = $derived(clampCell(chosen, { rows: rows.length, columns: columns.length }));

	// Start on `initialColumn`, scrolled to the left edge, whenever the columns change (a new month).
	$effect(() => {
		const column = Math.min(initialColumn, Math.max(0, columns.length - 1));
		chosen = { row: 0, column };
		if (viewport) viewport.scrollLeft = column * columnWidth;
	});

	// Columns in view, plus the active one wherever it is, so `aria-activedescendant` names a node.
	const rendered = $derived.by(() => {
		const visible = visibleColumns(
			scrollLeft,
			width - railWidth,
			columnWidth,
			columns.length,
			overscan
		);
		const inView = Array.from({ length: visible.end - visible.start }, (_, i) => visible.start + i);
		const outside = active.column < visible.start || active.column >= visible.end;
		return outside && active.column < columns.length ? [active.column, ...inView] : inView;
	});

	function cellId(cellAt: Cell): string {
		return `${id}-${cellAt.row}-${cellAt.column}`;
	}

	function activate(cellAt: Cell) {
		chosen = cellAt;
		const row = rows[cellAt.row];
		if (row) onactivate?.(row, columns[cellAt.column]);
	}

	function keydown(event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			activate(active);
			return;
		}
		const next = moveFocus(active, event.key, { rows: rows.length, columns: columns.length });
		if (!next || !viewport) return;
		event.preventDefault();
		chosen = next;
		viewport.scrollLeft = revealColumn(
			next.column,
			viewport.scrollLeft,
			viewport.clientWidth - railWidth,
			columnWidth
		);
	}
</script>

<div
	class="viewport"
	role="grid"
	tabindex="0"
	aria-label={label}
	aria-rowcount={rows.length + 1}
	aria-colcount={columns.length + 1}
	aria-activedescendant={rows.length > 0 && columns.length > 0 ? cellId(active) : undefined}
	bind:this={viewport}
	bind:clientWidth={width}
	onscroll={() => (scrollLeft = viewport?.scrollLeft ?? 0)}
	onkeydown={keydown}
>
	<div
		class="canvas"
		style:width="{railWidth + columns.length * columnWidth}px"
		style:--column="{columnWidth}px"
		style:--row="{rowHeight}px"
		style:--rail="{railWidth}px"
	>
		<div class="row header" role="row" aria-rowindex={1}>
			<div class="rail" role="columnheader" aria-colindex={1}></div>
			{#each rendered as column (columns[column])}
				<div
					class="cell"
					role="columnheader"
					aria-colindex={column + 2}
					style:transform="translateX({railWidth + column * columnWidth}px)"
				>
					{@render header(columns[column])}
				</div>
			{/each}
		</div>
		{#each rows as row, r (row.id)}
			<div class="row" role="row" aria-rowindex={r + 2}>
				<div class="rail" role="rowheader" aria-colindex={1}>{row.label}</div>
				{#each rendered as column (columns[column])}
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<!-- svelte-ignore a11y_interactive_supports_focus -->
					<div
						id={cellId({ row: r, column })}
						class="cell"
						class:active={active.row === r && active.column === column}
						role="gridcell"
						aria-colindex={column + 2}
						aria-label={cellLabel(row, columns[column])}
						style:transform="translateX({railWidth + column * columnWidth}px)"
						onmousedown={(event) => event.preventDefault()}
						onclick={() => {
							activate({ row: r, column });
							viewport?.focus();
						}}
					>
						{@render cell(row, columns[column])}
					</div>
				{/each}
			</div>
		{/each}
	</div>
</div>

<style>
	.viewport {
		overflow: auto;
		max-height: 70vh;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.viewport:focus-visible {
		outline: 2px solid var(--accent);
	}
	.canvas {
		position: relative;
		/* Day lines, one per column after the row labels. */
		background-image: linear-gradient(to right, var(--border) 1px, transparent 1px);
		background-size: var(--column) 100%;
		background-position: var(--rail) 0;
	}
	.row {
		position: relative;
		height: var(--row);
		border-bottom: 1px solid var(--border);
	}
	.header {
		position: sticky;
		top: 0;
		z-index: 2;
		background: var(--surface);
	}
	.rail {
		position: sticky;
		left: 0;
		z-index: 1;
		width: var(--rail);
		height: 100%;
		display: flex;
		align-items: center;
		padding: 0 0.5rem;
		background: var(--surface);
		border-right: 1px solid var(--border);
	}
	.cell {
		position: absolute;
		top: 0;
		left: 0;
		width: var(--column);
		height: var(--row);
		display: grid;
		place-items: center;
		cursor: pointer;
	}
	.cell.active {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
</style>
