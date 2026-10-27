import { afterEach, describe, expect, it, vi } from 'vitest';
import { rest } from './api/rest';
import {
	fiscalQuarterLabel,
	formatLkr,
	formatOccupancy,
	formatRupees,
	presets,
	recentFiscalQuarters,
	recentMonths,
	recentQuarters,
	recentVatPeriods,
	reportLink,
	tabTarget
} from './reports';

describe('presets', () => {
	// A Wednesday.
	const ranges = presets('2026-10-21');
	const range = (id: string) => ranges.find((entry) => entry.id === id);

	it('runs weeks from Monday to Sunday', () => {
		expect(range('this-week')).toMatchObject({ from: '2026-10-19', to: '2026-10-25' });
		expect(range('last-week')).toMatchObject({ from: '2026-10-12', to: '2026-10-18' });
	});

	it('gives whole months', () => {
		expect(range('this-month')).toMatchObject({ from: '2026-10-01', to: '2026-10-31' });
		expect(range('last-month')).toMatchObject({ from: '2026-09-01', to: '2026-09-30' });
	});

	it('takes a Sunday into the week that ends on it and a January into December', () => {
		expect(presets('2026-10-25').find((entry) => entry.id === 'this-week')).toMatchObject({
			from: '2026-10-19',
			to: '2026-10-25'
		});
		expect(presets('2027-01-05').find((entry) => entry.id === 'last-month')).toMatchObject({
			from: '2026-12-01',
			to: '2026-12-31'
		});
	});
});

describe('formatLkr', () => {
	it('shows base minor units (cents) as whole rupees', () => {
		expect(formatLkr(345351300)).toBe('Rs 3,453,513');
	});

	it('rounds to the rupee', () => {
		expect(formatLkr(12_050)).toBe('Rs 121');
		expect(formatLkr(0)).toBe('Rs 0');
	});

	it('keeps the sign of a refund', () => {
		expect(formatLkr(-250_000)).toBe('-Rs 2,500');
	});
});

describe('formatRupees', () => {
	it('shows whole rupees as they are, for the VAT report', () => {
		expect(formatRupees(3453513)).toBe('Rs 3,453,513');
	});
});

describe('formatOccupancy', () => {
	it('shows basis points with one decimal', () => {
		expect(formatOccupancy(7500)).toBe('75.0%');
		expect(formatOccupancy(6667)).toBe('66.7%');
	});
});

describe('fiscal quarters', () => {
	it('labels them as the form does, with the year of their months', () => {
		expect(fiscalQuarterLabel(2026, 1)).toBe('April – June (2026)');
		expect(fiscalQuarterLabel(2026, 4)).toBe('January – March (2027)');
	});

	it('lists the newest first, from the one the date falls in', () => {
		const quarters = recentFiscalQuarters('2026-02-10', 3);

		expect(quarters.map((entry) => [entry.fyStartYear, entry.quarter])).toEqual([
			[2025, 4],
			[2025, 3],
			[2025, 2]
		]);
		expect(quarters[0].label).toBe('January – March (2026)');
	});
});

describe('calendar quarters', () => {
	it('lists the newest first', () => {
		const quarters = recentQuarters('2026-02-10', 2);

		expect(quarters.map((entry) => [entry.year, entry.quarter])).toEqual([
			[2026, 1],
			[2025, 4]
		]);
		expect(quarters[1].label).toBe('October – December (2025)');
	});
});

describe('VAT periods', () => {
	it('codes a quarter YYQ0', () => {
		const periods = recentVatPeriods('2026-08-20', 'quarterly', 2);

		expect(periods.map((entry) => entry.code)).toEqual(['2630', '2620']);
		expect(periods[0].label).toBe('July – September (2026)');
	});

	it('codes a month YYQM, the month counted in its quarter', () => {
		const periods = recentVatPeriods('2026-08-20', 'monthly', 3);

		expect(periods.map((entry) => entry.code)).toEqual(['2632', '2631', '2623']);
		expect(periods[0].label).toBe('August 2026');
	});
});

describe('recentMonths', () => {
	it('lists the newest first across a year end', () => {
		expect(recentMonths('2026-01-15', 2).map((entry) => [entry.year, entry.month])).toEqual([
			[2026, 1],
			[2025, 12]
		]);
	});
});

describe('reportLink', () => {
	const ok = { response: new Response(null, { status: 200 }) };

	afterEach(() => {
		vi.restoreAllMocks();
		vi.useRealTimers();
	});

	it('renders, then polls every second until the link is ready', async () => {
		vi.useFakeTimers();
		vi.spyOn(rest, 'POST').mockResolvedValue({ ...ok, data: { id: 'r1' } } as never);
		const read = vi
			.spyOn(rest, 'GET')
			.mockResolvedValueOnce({ ...ok, data: { status: 'rendering' } } as never)
			.mockResolvedValueOnce({
				...ok,
				data: { status: 'ready', url: 'https://files/r1.pdf' }
			} as never);

		const link = reportLink('p1', {
			report: 'summary',
			params: { from: '2026-10-12', to: '2026-10-18' }
		});
		await vi.advanceTimersByTimeAsync(1000);

		await expect(link).resolves.toBe('https://files/r1.pdf');
		expect(read).toHaveBeenCalledTimes(2);
	});

	it('gives up after 30 polls, one a second', async () => {
		vi.useFakeTimers();
		vi.spyOn(rest, 'POST').mockResolvedValue({ ...ok, data: { id: 'r1' } } as never);
		const read = vi
			.spyOn(rest, 'GET')
			.mockResolvedValue({ ...ok, data: { status: 'rendering' } } as never);

		const link = reportLink('p1', {
			report: 'summary',
			params: { from: '2026-10-12', to: '2026-10-18' }
		});
		const failed = expect(link).rejects.toThrow('taking too long');
		await vi.advanceTimersByTimeAsync(60_000);

		await failed;
		expect(read).toHaveBeenCalledTimes(30);
	});

	it('stops polling when aborted', async () => {
		vi.useFakeTimers();
		vi.spyOn(rest, 'POST').mockResolvedValue({ ...ok, data: { id: 'r1' } } as never);
		const read = vi
			.spyOn(rest, 'GET')
			.mockResolvedValue({ ...ok, data: { status: 'rendering' } } as never);
		const controller = new AbortController();

		const link = reportLink(
			'p1',
			{ report: 'summary', params: { from: '2026-10-12', to: '2026-10-18' } },
			controller.signal
		);
		const failed = expect(link).rejects.toBeDefined();
		await vi.advanceTimersByTimeAsync(2000);
		controller.abort();
		await vi.advanceTimersByTimeAsync(10_000);

		await failed;
		expect(read).toHaveBeenCalledTimes(3);
	});

	it('fails when the render failed', async () => {
		vi.spyOn(rest, 'POST').mockResolvedValue({ ...ok, data: { id: 'r1' } } as never);
		vi.spyOn(rest, 'GET').mockResolvedValue({ ...ok, data: { status: 'failed' } } as never);

		await expect(
			reportLink('p1', { report: 'summary', params: { from: '2026-10-12', to: '2026-10-18' } })
		).rejects.toThrow('could not be rendered');
	});
});

describe('tabTarget', () => {
	it('moves with the arrows, wrapping, and jumps with Home and End', () => {
		expect(tabTarget('ArrowRight', 2, 3)).toBe(0);
		expect(tabTarget('ArrowLeft', 0, 3)).toBe(2);
		expect(tabTarget('Home', 2, 3)).toBe(0);
		expect(tabTarget('End', 0, 3)).toBe(2);
		expect(tabTarget('a', 0, 3)).toBeNull();
	});
});
