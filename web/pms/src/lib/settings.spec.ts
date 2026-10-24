import { describe, expect, it } from 'vitest';
import type { Profile } from './session';
import {
	bpToPercent,
	conditionLabel,
	isValidRate,
	parseCurrencies,
	percentToBp,
	rulePeriods,
	seesFxBanner,
	settingsAccess,
	type TaxRule
} from './settings';

function profile(grants: Profile['grants']): Profile {
	return { user_id: 'u', email: 'a@b.lk', display_name: 'A', tenants: [], grants };
}

describe('percentToBp', () => {
	it('reads a percentage with up to 2 decimals as exact basis points', () => {
		expect(percentToBp('18')).toBe(1800);
		expect(percentToBp('2.5')).toBe(250);
		expect(percentToBp('2.50')).toBe(250);
		expect(percentToBp('0.5')).toBe(50);
		expect(percentToBp('0.05')).toBe(5);
		expect(percentToBp('0')).toBe(0);
		expect(percentToBp(' 10 ')).toBe(1000);
		expect(percentToBp('100')).toBe(10_000);
		// 0.29 * 100 is 28.999999999999996 in floating point; digit arithmetic gives 29.
		expect(percentToBp('0.29')).toBe(29);
		expect(percentToBp('1.15')).toBe(115);
	});

	it('refuses more than 2 decimals, signs, separators, other text and more than 100%', () => {
		for (const text of [
			'2.555',
			'-1',
			'+1',
			'1,5',
			'1e2',
			'',
			'.5',
			'5.',
			'ten',
			'100.01',
			'101'
		]) {
			expect(percentToBp(text), text).toBeNull();
		}
	});

	it('round-trips through bpToPercent', () => {
		for (const bp of [0, 5, 29, 50, 100, 250, 1000, 1800, 1825, 10_000]) {
			expect(percentToBp(bpToPercent(bp))).toBe(bp);
		}
		expect(bpToPercent(250)).toBe('2.5');
		expect(bpToPercent(1800)).toBe('18');
		expect(bpToPercent(5)).toBe('0.05');
	});
});

describe('isValidRate', () => {
	it('takes positive rates with at most 6 decimals', () => {
		for (const text of ['301.25', '300', '0.000001', '123456789012.123456', '1.0']) {
			expect(isValidRate(text), text).toBe(true);
		}
	});

	it('refuses zero, negatives, more than 6 decimals, exponents and separators', () => {
		for (const text of [
			'0',
			'0.000000',
			'-1',
			'1.1234567',
			'1e3',
			'1,000',
			'',
			' 1',
			'.5',
			'5.',
			'1_000',
			'1234567890123'
		]) {
			expect(isValidRate(text), text).toBe(false);
		}
	});
});

describe('settingsAccess', () => {
	const pages = (grants: Profile['grants']) =>
		settingsAccess(profile(grants), 'p1').map(
			({ page, edit }) => `${page}:${edit ? 'edit' : 'view'}`
		);

	it('lets the owner change taxes, outlets, exchange rates and laundry prices', () => {
		expect(pages([{ role: 'owner' }])).toEqual([
			'taxes:edit',
			'outlets:edit',
			'exchange-rates:edit',
			'laundry-prices:edit'
		]);
	});

	it('lets a manager set exchange rates and laundry prices and only read taxes and outlets', () => {
		expect(pages([{ role: 'manager', property_id: 'p1' }])).toEqual([
			'taxes:view',
			'outlets:view',
			'exchange-rates:edit',
			'laundry-prices:edit'
		]);
	});

	it('shows front desk and accountants every page read-only', () => {
		for (const role of ['front_desk', 'accountant'] as const) {
			expect(pages([{ role }])).toEqual(['taxes:view', 'outlets:view', 'exchange-rates:view']);
		}
	});

	it('shows nothing without FolioView, or at another property', () => {
		expect(pages([{ role: 'housekeeping' }])).toEqual([]);
		expect(pages([{ role: 'hk_supervisor' }])).toEqual([]);
		expect(pages([{ role: 'owner', property_id: 'p2' }])).toEqual([]);
	});
});

describe('seesFxBanner', () => {
	it('is shown to owners and managers only', () => {
		expect(seesFxBanner(profile([{ role: 'owner' }]), 'p1')).toBe(true);
		expect(seesFxBanner(profile([{ role: 'manager', property_id: 'p1' }]), 'p1')).toBe(true);
		expect(seesFxBanner(profile([{ role: 'manager', property_id: 'p2' }]), 'p1')).toBe(false);
		for (const role of ['front_desk', 'accountant', 'housekeeping'] as const) {
			expect(seesFxBanner(profile([{ role }]), 'p1')).toBe(false);
		}
	});
});

describe('rulePeriods', () => {
	function rule(code: string, sequence: number, from: string, to: string | null): TaxRule {
		return {
			id: `${code}-${from}`,
			code,
			name: code,
			kind: 'LEVY',
			rateBp: 100,
			appliesTo: ['*'],
			base: ['net'],
			sequence,
			presentation: 'ON_BILL',
			condition: { type: 'always' },
			effectiveFrom: from,
			effectiveTo: to,
			version: 1
		};
	}

	it('puts the periods in force first, then those to come, then those ended, each in sequence', () => {
		const periods = rulePeriods(
			[
				rule('VAT', 40, '2026-01-01', '2026-10-19'),
				rule('TDL', 20, '2026-01-01', '2026-10-19'),
				rule('OLD', 10, '2025-01-01', '2026-01-01'),
				rule('TDL', 20, '2026-10-19', null),
				rule('OLDER', 10, '2024-01-01', '2025-01-01'),
				rule('LATER', 10, '2027-01-01', null)
			],
			'2026-10-18'
		);
		expect(periods.map((p) => [p.from, p.state, p.rules.map((r) => r.code)])).toEqual([
			['2026-01-01', 'current', ['TDL', 'VAT']],
			['2026-10-19', 'upcoming', ['TDL']],
			['2027-01-01', 'upcoming', ['LATER']],
			['2025-01-01', 'ended', ['OLD']],
			['2024-01-01', 'ended', ['OLDER']]
		]);
	});

	it('counts a rule ending on the business date as ended and one starting on it as current', () => {
		const periods = rulePeriods(
			[rule('A', 10, '2026-01-01', '2026-10-18'), rule('B', 10, '2026-10-18', null)],
			'2026-10-18'
		);
		expect(periods.map((p) => [p.rules[0].code, p.state])).toEqual([
			['B', 'current'],
			['A', 'ended']
		]);
	});
});

describe('conditionLabel', () => {
	it('names each condition', () => {
		expect(conditionLabel({ type: 'always' })).toBe('Always');
		expect(conditionLabel({ type: 'vat_registered' })).toBe('When VAT-registered');
		expect(conditionLabel({ type: 'residency', resident: false })).toBe('Non-residents only');
		expect(conditionLabel({ type: 'payment_currency', currencies: ['USD', 'EUR'] })).toBe(
			'Paid in USD, EUR'
		);
	});
});

describe('parseCurrencies', () => {
	it('reads codes separated by commas or spaces, upper-cased and each once', () => {
		expect(parseCurrencies('lkr, USD  eur,,USD')).toEqual(['LKR', 'USD', 'EUR']);
		expect(parseCurrencies('  ')).toEqual([]);
	});
});
