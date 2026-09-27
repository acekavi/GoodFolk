/**
 * Layout math for virtualizing fixed-size runs: horizontally for date grids (the inventory calendar now,
 * the tape chart later), vertically for long fixed-row-height tables (the reservations list). Only the
 * indices returned by `visibleColumns` (or its `visibleRows` alias) are rendered as DOM nodes.
 */

/** Columns `[start, end)` to render; the same shape reused for rows `[start, end)`. */
export interface ColumnWindow {
	start: number;
	end: number;
}

/**
 * The items intersecting the viewport, plus `overscan` items on each side. Takes the scroll offset,
 * viewport size and item size along one axis, so it works unchanged for either a horizontal date grid's
 * columns or a vertical table's rows.
 */
export function visibleColumns(
	scrollLeft: number,
	viewportWidth: number,
	columnWidth: number,
	columnCount: number,
	overscan = 2
): ColumnWindow {
	const first = Math.floor(scrollLeft / columnWidth);
	const last = Math.ceil((scrollLeft + viewportWidth) / columnWidth);
	return {
		start: Math.max(0, Math.min(columnCount, first - overscan)),
		end: Math.max(0, Math.min(columnCount, last + overscan))
	};
}

/** `visibleColumns`, named for a vertical list: the rows intersecting the viewport plus `overscan`. */
export const visibleRows = visibleColumns;

/** The scroll position that shows `column` with as little movement as possible. */
export function revealColumn(
	column: number,
	scrollLeft: number,
	viewportWidth: number,
	columnWidth: number
): number {
	const left = column * columnWidth;
	const right = left + columnWidth;
	if (left < scrollLeft) return left;
	if (right > scrollLeft + viewportWidth) return right - viewportWidth;
	return scrollLeft;
}

/** `revealColumn`, named for a vertical list: the scroll position that shows `row`. */
export const revealRow = revealColumn;

export interface Cell {
	row: number;
	column: number;
}

/**
 * The current index of the row with id `chosenId`, or `fallbackIndex` clamped to the rows that exist when
 * that row is no longer among `rows` (its type was retired). Resolving by id, rather than by position,
 * keeps the chosen row in place when a different row is removed or restored around it.
 */
export function resolveRow(
	rows: { id: string }[],
	chosenId: string | null,
	fallbackIndex: number
): number {
	if (chosenId !== null) {
		const index = rows.findIndex((row) => row.id === chosenId);
		if (index !== -1) return index;
	}
	return Math.max(0, Math.min(rows.length - 1, fallbackIndex));
}

/** `cell`, moved onto the last row or column if the grid no longer has it (`{ 0, 0 }` when empty). */
export function clampCell(cell: Cell, size: { rows: number; columns: number }): Cell {
	return {
		row: Math.max(0, Math.min(size.rows - 1, cell.row)),
		column: Math.max(0, Math.min(size.columns - 1, cell.column))
	};
}

/** The cell a navigation key moves to, or `null` if the key does not navigate. */
export function moveFocus(
	cell: Cell,
	key: string,
	size: { rows: number; columns: number }
): Cell | null {
	const clamp = (value: number, count: number) => Math.max(0, Math.min(count - 1, value));
	switch (key) {
		case 'ArrowLeft':
			return { ...cell, column: clamp(cell.column - 1, size.columns) };
		case 'ArrowRight':
			return { ...cell, column: clamp(cell.column + 1, size.columns) };
		case 'ArrowUp':
			return { ...cell, row: clamp(cell.row - 1, size.rows) };
		case 'ArrowDown':
			return { ...cell, row: clamp(cell.row + 1, size.rows) };
		case 'Home':
			return { ...cell, column: 0 };
		case 'End':
			return { ...cell, column: size.columns - 1 };
		default:
			return null;
	}
}
