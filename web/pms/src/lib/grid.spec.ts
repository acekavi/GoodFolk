import { describe, expect, it } from 'vitest';
import { clampCell, moveFocus, resolveRow, revealColumn, visibleColumns } from './grid';

describe('visibleColumns', () => {
	it('covers the columns in view plus overscan on both sides', () => {
		// 56 px columns, scrolled 3.5 columns in, 5 columns wide.
		expect(visibleColumns(196, 280, 56, 31, 1)).toEqual({ start: 2, end: 10 });
	});

	it('is clamped to the columns that exist', () => {
		expect(visibleColumns(0, 280, 56, 31, 2)).toEqual({ start: 0, end: 7 });
		expect(visibleColumns(56 * 29, 280, 56, 31, 2)).toEqual({ start: 27, end: 31 });
		expect(visibleColumns(0, 280, 56, 0, 2)).toEqual({ start: 0, end: 0 });
	});
});

describe('revealColumn', () => {
	it('scrolls just enough to show a column that is out of view', () => {
		expect(revealColumn(10, 0, 280, 56)).toBe(11 * 56 - 280);
		expect(revealColumn(1, 5 * 56, 280, 56)).toBe(56);
	});

	it('leaves the scroll position alone when the column is visible', () => {
		expect(revealColumn(3, 56, 280, 56)).toBe(56);
	});
});

describe('moveFocus', () => {
	const size = { rows: 3, columns: 31 };

	it('moves one cell with the arrow keys and stops at the edges', () => {
		expect(moveFocus({ row: 1, column: 5 }, 'ArrowRight', size)).toEqual({ row: 1, column: 6 });
		expect(moveFocus({ row: 1, column: 5 }, 'ArrowUp', size)).toEqual({ row: 0, column: 5 });
		expect(moveFocus({ row: 0, column: 0 }, 'ArrowLeft', size)).toEqual({ row: 0, column: 0 });
		expect(moveFocus({ row: 2, column: 30 }, 'ArrowDown', size)).toEqual({ row: 2, column: 30 });
	});

	it('jumps to the first or last day with Home and End', () => {
		expect(moveFocus({ row: 1, column: 5 }, 'Home', size)).toEqual({ row: 1, column: 0 });
		expect(moveFocus({ row: 1, column: 5 }, 'End', size)).toEqual({ row: 1, column: 30 });
	});

	it('ignores other keys', () => {
		expect(moveFocus({ row: 1, column: 5 }, 'a', size)).toBeNull();
	});
});

describe('clampCell', () => {
	it('keeps a cell that still exists', () => {
		expect(clampCell({ row: 1, column: 5 }, { rows: 3, columns: 31 })).toEqual({
			row: 1,
			column: 5
		});
	});

	it('moves to the last row or column when the grid shrinks', () => {
		expect(clampCell({ row: 2, column: 30 }, { rows: 2, columns: 28 })).toEqual({
			row: 1,
			column: 27
		});
	});

	it('stays at the origin of an empty grid', () => {
		expect(clampCell({ row: 2, column: 5 }, { rows: 0, columns: 0 })).toEqual({
			row: 0,
			column: 0
		});
	});
});

describe('resolveRow', () => {
	it('keeps the chosen row when a different row is removed and then restored', () => {
		const rows = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
		expect(resolveRow(rows, 'c', 2)).toBe(2);

		const withoutB = [{ id: 'a' }, { id: 'c' }];
		expect(resolveRow(withoutB, 'c', 2)).toBe(1);

		expect(resolveRow(rows, 'c', 1)).toBe(2);
	});

	it('falls back to the clamped index when the chosen row itself is gone', () => {
		const withoutB = [{ id: 'a' }, { id: 'c' }];
		expect(resolveRow(withoutB, 'b', 1)).toBe(1);
	});

	it('stays at the origin when there are no rows', () => {
		expect(resolveRow([], 'b', 1)).toBe(0);
	});
});
