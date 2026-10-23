import { graphql } from './api/gql';
import type {
	AuditRunsQuery,
	AuditStep,
	DailyStatsQuery,
	NightAuditQuery,
	OpsAlertsQuery
} from './api/gql/graphql';
import { query } from './api/graphql';
import { methodLabel, money, PAYMENT_METHODS } from './folio';
import { can, type Profile } from './session';

/** The Night audit page's review of the business date. */
export const NightAuditDocument = graphql(`
	query NightAudit($propertyId: UUID!) {
		nightAudit(propertyId: $propertyId) {
			businessDate
			blocking
			arrivals {
				reservationRoomId
				reservationId
				confirmation
				guest
				roomNumber
				arrival
				departure
				status
				total
				currency
				noShowPenalty
				version
			}
			departures {
				reservationRoomId
				reservationId
				confirmation
				guest
				roomNumber
				departure
				balances {
					currency
					amount
				}
				version
			}
			posBills {
				outlet
				totalBase
				bills {
					id
					billNo
					currency
					total
					totalBase
					roomNumber
					settled
					highlight
				}
			}
			bookingBills {
				nights {
					reservationRoomId
					roomNumber
					date
					roomAmount
					mealAmount
					currency
					problem
					alreadyPosted
				}
				posted {
					category
					lines
					totalBase
				}
			}
			payments {
				method
				currency
				count
				amount
				baseAmount
				refunds
				flagged
			}
		}
	}
`);

/** The newest closes, each with its steps: the Night audit page's tracker and the Audit history. */
export const AuditRunsDocument = graphql(`
	query AuditRuns($propertyId: UUID!, $first: Int!) {
		auditRuns(propertyId: $propertyId, first: $first) {
			id
			businessDate
			mode
			status
			startedBy
			attempts
			error
			failure
			startedAt
			finishedAt
			steps {
				step
				completedAt
				detail
			}
		}
	}
`);

/** The Audit history's day table. */
export const DailyStatsDocument = graphql(`
	query DailyStats($propertyId: UUID!, $from: Date!, $to: Date!) {
		dailyStats(propertyId: $propertyId, from: $from, to: $to) {
			businessDate
			roomsAvailable
			roomsSold
			occupancyBp
			arrivals
			departures
			roomRevenue
			mealPlanRevenue
			posRevenue
			feeRevenue
			otherRevenue
			adr
			revpar
		}
	}
`);

/** The banner's ops alerts. */
export const OpsAlertsDocument = graphql(`
	query OpsAlerts($propertyId: UUID!) {
		opsAlerts(propertyId: $propertyId) {
			kind
			message
			raisedAt
		}
	}
`);

export type NightAudit = NightAuditQuery['nightAudit'];
export type Arrival = NightAudit['arrivals'][number];
export type Departure = NightAudit['departures'][number];
export type PaymentGroup = NightAudit['payments'][number];
export type AuditRun = AuditRunsQuery['auditRuns'][number];
export type DailyStats = DailyStatsQuery['dailyStats'][number];
export type OpsAlert = OpsAlertsQuery['opsAlerts'][number];

/** A row action of the review that `AuditActionDialog` confirms. */
export type AuditAction =
	{ kind: 'noShow' | 'delay'; row: Arrival } | { kind: 'checkOut' | 'extend'; row: Departure };

/** Refetched on the server's `audit:<property>` event (see `events.ts`), and on `reservations:<property>`. */
export function nightAuditKey(propertyId: string) {
	return ['night-audit', propertyId] as const;
}

/** Refetched on `audit:<property>`; `auditRunsKey(propertyId)` alone is the prefix of every page size. */
export function auditRunsKey(propertyId: string, ...first: [first: number] | []) {
	return ['audit-runs', propertyId, ...first] as const;
}

/** Refetched on `audit:<property>`; `dailyStatsKey(propertyId)` alone is the prefix of every range. */
export function dailyStatsKey(propertyId: string, ...range: [from: string, to: string] | []) {
	return ['daily-stats', propertyId, ...range] as const;
}

/** Refetched on `audit:<property>`, which a raised or cleared alert queues. */
export function opsAlertsKey(propertyId: string) {
	return ['ops-alerts', propertyId] as const;
}

/** Every query the server's `audit:<property>` event names: the review, the runs, the statistics and the alerts. */
export function auditKeys(propertyId: string) {
	return [
		nightAuditKey(propertyId),
		auditRunsKey(propertyId),
		dailyStatsKey(propertyId),
		opsAlertsKey(propertyId)
	];
}

export async function fetchNightAudit(propertyId: string, signal?: AbortSignal) {
	return (await query(NightAuditDocument, { propertyId }, signal)).nightAudit;
}

export async function fetchAuditRuns(propertyId: string, first: number, signal?: AbortSignal) {
	return (await query(AuditRunsDocument, { propertyId, first }, signal)).auditRuns;
}

/** The statistics of the closed dates from `from` to `to`, both included. */
export async function fetchDailyStats(
	propertyId: string,
	from: string,
	to: string,
	signal?: AbortSignal
) {
	return (await query(DailyStatsDocument, { propertyId, from, to }, signal)).dailyStats;
}

export async function fetchOpsAlerts(propertyId: string, signal?: AbortSignal) {
	return (await query(OpsAlertsDocument, { propertyId }, signal)).opsAlerts;
}

/** Whether the ops-alert banner is shown: to owners and managers, who can read the alerts. */
export function seesOpsAlerts(profile: Profile, propertyId: string): boolean {
	return can(profile, 'propertiesManage', propertyId);
}

export function opsAlertLabel(kind: OpsAlert['kind']): string {
	return kind === 'AUDIT' ? 'Night audit' : 'Inventory counts';
}

/** The header's count of stays still blocking the close. */
export function blockingText(blocking: number): string {
	return blocking === 0 ? 'Nothing to resolve' : `${blocking} to resolve`;
}

/** A payment method as the review shows it: a charge to account is the city ledger. */
export function auditMethodLabel(method: string): string {
	return method === 'account' ? 'City ledger' : methodLabel(method);
}

export interface MethodPayments {
	method: string;
	label: string;
	groups: PaymentGroup[];
}

/** The day's payment groups by method, in the payment form's order (any other method after), each by currency. */
export function paymentsByMethod(groups: readonly PaymentGroup[]): MethodPayments[] {
	const byMethod = new Map<string, PaymentGroup[]>();
	for (const group of groups) {
		const list = byMethod.get(group.method) ?? [];
		list.push(group);
		byMethod.set(group.method, list);
	}
	const rank = (method: string) => {
		const index = (PAYMENT_METHODS as readonly string[]).indexOf(method);
		return index === -1 ? PAYMENT_METHODS.length : index;
	};
	return [...byMethod.entries()]
		.sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
		.map(([method, list]) => ({
			method,
			label: auditMethodLabel(method),
			groups: list.sort((a, b) => a.currency.localeCompare(b.currency))
		}));
}

/**
 * How much of a stay's night is already posted. The night has a room charge, plus a meal-plan charge when its meal
 * amount is above zero, as the server counts them.
 */
export function nightPosting(
	night: Pick<NightAudit['bookingBills']['nights'][number], 'mealAmount' | 'alreadyPosted'>
): 'posted' | 'partly' | 'none' {
	const charges = 1 + (night.mealAmount > 0 ? 1 : 0);
	if (night.alreadyPosted >= charges) return 'posted';
	return night.alreadyPosted > 0 ? 'partly' : 'none';
}

/** The balances not at zero, as `LKR 1,000.00, USD -5.00`; `null` when the folio is settled. */
export function owedText(balances: readonly { currency: string; amount: number }[]): string | null {
	const open = balances.filter((balance) => balance.amount !== 0);
	if (open.length === 0) return null;
	return open.map((balance) => money(balance.amount, balance.currency)).join(', ');
}

/** The steps of a close, in the order they run. */
export const AUDIT_STEPS: readonly AuditStep[] = [
	'RESOLVE',
	'POST_CHARGES',
	'LOCK_FX',
	'SNAPSHOT_STATS',
	'ADVANCE_BUSINESS_DATE'
];

const STEP_LABELS: Record<AuditStep, string> = {
	RESOLVE: 'Resolve arrivals and departures',
	POST_CHARGES: "Post tonight's charges",
	LOCK_FX: 'Lock exchange rates',
	SNAPSHOT_STATS: 'Snapshot statistics',
	ADVANCE_BUSINESS_DATE: 'Advance the business date'
};

export function stepLabel(step: AuditStep): string {
	return STEP_LABELS[step];
}

export type StepState = 'pending' | 'running' | 'done' | 'failed';

function record(value: unknown): Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: {};
}

/** The step a failed run's report names (`{"step": "post_charges"}`), or `null`. */
function failedStep(run: Pick<AuditRun, 'failure'>): AuditStep | null {
	const step = record(run.failure).step;
	if (typeof step !== 'string') return null;
	const upper = step.toUpperCase() as AuditStep;
	return AUDIT_STEPS.includes(upper) ? upper : null;
}

/**
 * Each step of the run, in order, with its state: done when the run recorded it; the first step not done is
 * running while the run is, or failed when it failed (the step its report names, if it names one); the rest are
 * pending.
 */
export function stepStates(
	run: Pick<AuditRun, 'status' | 'steps' | 'failure'>
): { step: AuditStep; label: string; state: StepState }[] {
	const done = new Set(run.steps.map((entry) => entry.step));
	const next = AUDIT_STEPS.find((step) => !done.has(step));
	const failed = run.status === 'FAILED' ? (failedStep(run) ?? next) : null;
	return AUDIT_STEPS.map((step) => {
		let state: StepState = 'pending';
		if (done.has(step)) state = 'done';
		else if (step === failed) state = 'failed';
		else if (run.status === 'RUNNING' && step === next) state = 'running';
		return { step, label: stepLabel(step), state };
	});
}

/** The stays a failed step names in its report: their room (when assigned) and why each failed. */
export function failedStays(
	run: Pick<AuditRun, 'failure'>
): { reservationRoom: string; room: string | null; error: string }[] {
	const stays = record(run.failure).stays;
	if (!Array.isArray(stays)) return [];
	return stays.map((entry) => {
		const stay = record(entry);
		return {
			reservationRoom: typeof stay.reservation_room === 'string' ? stay.reservation_room : '',
			room: typeof stay.room === 'string' ? stay.room : null,
			error: typeof stay.error === 'string' ? stay.error : 'Failed'
		};
	});
}

/** A step's report in words: `stays: 3, lines: 6`; a list reads as its length. */
export function stepDetailText(detail: unknown): string {
	return Object.entries(record(detail))
		.map(([key, value]) => {
			const shown = Array.isArray(value)
				? String(value.length)
				: typeof value === 'object' && value !== null
					? JSON.stringify(value)
					: String(value);
			return `${key.replaceAll('_', ' ')}: ${shown}`;
		})
		.join(', ');
}

/** How long a run took, as `1 min 5 s` or `12 s`; `null` while it has not finished. */
export function runDuration(run: Pick<AuditRun, 'startedAt' | 'finishedAt'>): string | null {
	if (!run.finishedAt) return null;
	const seconds = Math.max(
		0,
		Math.round((Date.parse(run.finishedAt) - Date.parse(run.startedAt)) / 1000)
	);
	const minutes = Math.floor(seconds / 60);
	return minutes > 0 ? `${minutes} min ${seconds % 60} s` : `${seconds} s`;
}

/** Basis points as a percentage with one decimal: 7550 is `75.5%`. */
export function occupancyText(bp: number): string {
	return `${(bp / 100).toFixed(1)}%`;
}

/** A day's revenue net of tax: rooms, meal plans, POS, fees and the rest. */
export function dayRevenue(
	stats: Pick<
		DailyStats,
		'roomRevenue' | 'mealPlanRevenue' | 'posRevenue' | 'feeRevenue' | 'otherRevenue'
	>
): number {
	return (
		stats.roomRevenue +
		stats.mealPlanRevenue +
		stats.posRevenue +
		stats.feeRevenue +
		stats.otherRevenue
	);
}

export interface Night {
	date: string;
	room: number;
	meal: number;
}

/** What the stay's last night cost, room and meal, which an extension charges again; `null` with no nights. */
export function lastNightAmount(nights: readonly Night[]): number | null {
	if (nights.length === 0) return null;
	const last = nights.reduce((a, b) => (b.date > a.date ? b : a));
	return last.room + last.meal;
}

/** The stay's total once its arrival moves to `newCheckIn`: the nights before it are released. */
export function totalAfterDelay(nights: readonly Night[], newCheckIn: string): number {
	return nights
		.filter((night) => night.date >= newCheckIn)
		.reduce((sum, night) => sum + night.room + night.meal, 0);
}
