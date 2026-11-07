import { describe, expect, it } from 'vitest';
import { addMonths, dateText, guestsText, money, nights, totalsText } from './format';

describe('money', () => {
	it('shows rupees as whole rupees with the Rs symbol', () => {
		expect(money(1_234_500, 'LKR')).toBe('Rs 12,345');
		expect(money(0, 'LKR')).toBe('Rs 0');
	});

	it('keeps the cents of a rupee amount that has them, never rounding', () => {
		expect(money(1_234_550, 'LKR')).toBe('Rs 12,345.50');
	});

	it('shows US dollars with the US$ symbol and both decimals', () => {
		expect(money(12_345, 'USD')).toBe('US$ 123.45');
		expect(money(12_300, 'USD')).toBe('US$ 123.00');
		expect(money(5, 'USD')).toBe('US$ 0.05');
	});

	it('shows any other currency by its code with its own minor digits', () => {
		expect(money(12_345, 'EUR')).toBe('EUR 123.45');
		expect(money(1_234, 'JPY')).toBe('JPY 1,234');
		expect(money(1_234_567, 'KWD')).toBe('KWD 1,234.567');
	});

	it('puts the sign before the symbol', () => {
		expect(money(-250_000, 'LKR')).toBe('-Rs 2,500');
	});
});

describe('totalsText', () => {
	it('lists a total per currency', () => {
		expect(totalsText({ LKR: 3_000_000, USD: 24_000 })).toBe('Rs 30,000 + US$ 240.00');
	});
});

describe('dates', () => {
	it('shows a stay date as the calendar day it is, whatever the visitor’s time zone', () => {
		expect(dateText('2026-11-03')).toBe('Tue, 3 Nov 2026');
	});

	it('counts nights across a month end', () => {
		expect(nights('2026-10-30', '2026-11-02')).toBe(3);
	});

	it('adds months as the server does, to the month’s last day when it is shorter', () => {
		expect(addMonths('2026-08-31', 18)).toBe('2028-02-29');
		expect(addMonths('2026-11-15', 18)).toBe('2028-05-15');
	});

	it('names the guests of a room', () => {
		expect(guestsText(2, 0)).toBe('2 adults');
		expect(guestsText(1, 1)).toBe('1 adult, 1 child');
	});
});
