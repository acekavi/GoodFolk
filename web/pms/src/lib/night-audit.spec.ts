import { describe, expect, it } from 'vitest';
import type { Profile } from './session';
import {
	auditMethodLabel,
	blockingText,
	dayRevenue,
	failedStays,
	lastNightAmount,
	nightPosting,
	occupancyText,
	owedText,
	paymentsByMethod,
	runDuration,
	seesOpsAlerts,
	stepDetailText,
	stepStates,
	totalAfterDelay,
	type AuditRun,
	type PaymentGroup
} from './night-audit';

function group(method: string, currency: string): PaymentGroup {
	return { method, currency, count: 1, amount: 100, baseAmount: 100, refunds: 0, flagged: [] };
}

type RunState = Pick<AuditRun, 'status' | 'steps' | 'failure'>;

function run(status: RunState['status'], done: number, failure: unknown = null): RunState {
	const steps = (['RESOLVE', 'POST_CHARGES', 'LOCK_FX', 'SNAPSHOT_STATS'] as const)
		.slice(0, done)
		.map((step) => ({ step, completedAt: '2026-10-20T18:00:00Z', detail: {} }));
	return { status, steps, failure };
}

describe('paymentsByMethod', () => {
	it('groups the day by method in the payment form order, each by currency', () => {
		const grouped = paymentsByMethod([
			group('account', 'LKR'),
			group('cash', 'USD'),
			group('voucher', 'LKR'),
			group('cash', 'LKR'),
			group('card_terminal', 'LKR')
		]);

		expect(grouped.map((entry) => entry.method)).toEqual([
			'cash',
			'card_terminal',
			'account',
			'voucher'
		]);
		expect(grouped[0].groups.map((entry) => entry.currency)).toEqual(['LKR', 'USD']);
		expect(grouped.map((entry) => entry.label)).toEqual(['Cash', 'Card', 'City ledger', 'voucher']);
	});

	it('is empty for a day without payments', () => {
		expect(paymentsByMethod([])).toEqual([]);
	});
});

describe('auditMethodLabel', () => {
	it('shows a charge to account as the city ledger, and other methods as the folio does', () => {
		expect(auditMethodLabel('account')).toBe('City ledger');
		expect(auditMethodLabel('bank_transfer')).toBe('Bank transfer');
	});
});

describe('blockingText', () => {
	it('counts what is left to resolve', () => {
		expect(blockingText(3)).toBe('3 to resolve');
		expect(blockingText(1)).toBe('1 to resolve');
		expect(blockingText(0)).toBe('Nothing to resolve');
	});
});

describe('stepStates', () => {
	const states = (value: RunState) => stepStates(value).map((entry) => entry.state);

	it('marks the steps done, the next one running and the rest pending while the run runs', () => {
		expect(states(run('RUNNING', 2))).toEqual(['done', 'done', 'running', 'pending', 'pending']);
		expect(states(run('RUNNING', 0))).toEqual([
			'running',
			'pending',
			'pending',
			'pending',
			'pending'
		]);
	});

	it('marks the step the failure names as failed', () => {
		expect(states(run('FAILED', 1, { step: 'post_charges' }))).toEqual([
			'done',
			'failed',
			'pending',
			'pending',
			'pending'
		]);
	});

	it('marks the first step not done as failed when the report names none', () => {
		expect(states(run('FAILED', 3, {}))).toEqual(['done', 'done', 'done', 'failed', 'pending']);
	});

	it('marks every step done once the run completed', () => {
		const completed: RunState = {
			...run('COMPLETED', 4),
			steps: [
				...run('COMPLETED', 4).steps,
				{ step: 'ADVANCE_BUSINESS_DATE', completedAt: '2026-10-20T18:00:01Z', detail: {} }
			]
		};
		expect(states(completed)).toEqual(['done', 'done', 'done', 'done', 'done']);
		expect(stepStates(completed).map((entry) => entry.label)[4]).toBe('Advance the business date');
	});
});

describe('failedStays', () => {
	it('reads the stays of a failed report, with or without a room', () => {
		expect(
			failedStays({
				failure: {
					step: 'post_charges',
					stays: [
						{ reservation_room: 'rr1', room: '101', error: 'no USD rate' },
						{ reservation_room: 'rr2', room: null, error: 'no rate' }
					]
				}
			})
		).toEqual([
			{ reservationRoom: 'rr1', room: '101', error: 'no USD rate' },
			{ reservationRoom: 'rr2', room: null, error: 'no rate' }
		]);
		expect(failedStays({ failure: { step: 'lock_fx' } })).toEqual([]);
		expect(failedStays({ failure: null })).toEqual([]);
	});
});

describe('stepDetailText', () => {
	it('reads a report as its keys, a list as its length', () => {
		expect(stepDetailText({ stays: 3, lines: 6, skipped_existing: 0 })).toBe(
			'stays: 3, lines: 6, skipped existing: 0'
		);
		expect(stepDetailText({ locked: [{ currency: 'USD' }] })).toBe('locked: 1');
		expect(stepDetailText(null)).toBe('');
	});
});

describe('runDuration', () => {
	it('is the time from start to finish, and nothing while running', () => {
		expect(
			runDuration({ startedAt: '2026-10-20T18:00:00Z', finishedAt: '2026-10-20T18:01:05Z' })
		).toBe('1 min 5 s');
		expect(
			runDuration({ startedAt: '2026-10-20T18:00:00Z', finishedAt: '2026-10-20T18:00:12Z' })
		).toBe('12 s');
		expect(runDuration({ startedAt: '2026-10-20T18:00:00Z', finishedAt: null })).toBeNull();
	});
});

describe('price effects', () => {
	const nights = [
		{ date: '2026-10-20', room: 10_000, meal: 2_000 },
		{ date: '2026-10-21', room: 11_000, meal: 2_000 }
	];

	it('charges an extension what the last night cost', () => {
		expect(lastNightAmount(nights)).toBe(13_000);
		expect(lastNightAmount([])).toBeNull();
	});

	it('releases the nights before the delayed arrival', () => {
		expect(totalAfterDelay(nights, '2026-10-21')).toBe(13_000);
	});
});

describe('daily statistics', () => {
	it('shows occupancy as a percentage with one decimal', () => {
		expect(occupancyText(7550)).toBe('75.5%');
		expect(occupancyText(10_000)).toBe('100.0%');
	});

	it('sums the day revenue across rooms, meal plans, POS, fees and the rest', () => {
		expect(
			dayRevenue({
				roomRevenue: 1,
				mealPlanRevenue: 2,
				posRevenue: 3,
				feeRevenue: 4,
				otherRevenue: 5
			})
		).toBe(15);
	});
});

describe('seesOpsAlerts', () => {
	const profile = (role: Profile['grants'][number]['role']): Profile => ({
		user_id: 'u',
		email: 'a@b.lk',
		display_name: 'A',
		tenants: [],
		grants: [{ role, property_id: null }]
	});

	it('shows the banner to owners and managers only', () => {
		expect(seesOpsAlerts(profile('owner'), 'p1')).toBe(true);
		expect(seesOpsAlerts(profile('manager'), 'p1')).toBe(true);
		expect(seesOpsAlerts(profile('front_desk'), 'p1')).toBe(false);
		expect(seesOpsAlerts(profile('accountant'), 'p1')).toBe(false);
	});
});

describe('nightPosting', () => {
	it('reads a room-only night with its room charge posted as posted', () => {
		expect(nightPosting({ mealAmount: 0, alreadyPosted: 1 })).toBe('posted');
	});

	it('reads a night with meals and one of its two charges posted as partly posted', () => {
		expect(nightPosting({ mealAmount: 500, alreadyPosted: 1 })).toBe('partly');
		expect(nightPosting({ mealAmount: 500, alreadyPosted: 2 })).toBe('posted');
	});

	it('reads a night with nothing posted as to post', () => {
		expect(nightPosting({ mealAmount: 0, alreadyPosted: 0 })).toBe('none');
		expect(nightPosting({ mealAmount: 500, alreadyPosted: 0 })).toBe('none');
	});
});

describe('owedText', () => {
	it('joins the balances not at zero, and is null when settled', () => {
		expect(
			owedText([
				{ currency: 'LKR', amount: 100_000 },
				{ currency: 'EUR', amount: 0 },
				{ currency: 'USD', amount: -500 }
			])
		).toBe('LKR 1,000.00, USD -5.00');
		expect(owedText([{ currency: 'LKR', amount: 0 }])).toBeNull();
		expect(owedText([])).toBeNull();
	});
});
