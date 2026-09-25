/**
 * Layout math for horizontally virtualized date grids: the inventory calendar now, the tape chart later.
 * Only the columns returned by `visibleColumns` are rendered as DOM nodes.
 */

/** Columns `[start, end)` to render. */
export interface ColumnWindow {
	start: number;
	end: number;
}

/** The columns intersecting the viewport, plus `overscan` columns on each side. */
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

export interface Cell {
	row: number;
	column: number;
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
