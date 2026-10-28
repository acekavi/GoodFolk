import { graphql } from './api/gql';
import type {
	DailyListKind,
	DailyListQuery,
	ReportEmailsQuery,
	ReportSummaryQuery,
	ServiceChargeReportQuery,
	SsclReportQuery,
	StoredReportKind,
	TdlReportQuery,
	VatReportQuery
} from './api/gql/graphql';
import { query } from './api/graphql';
import type { components } from './api/openapi';
import { ApiError } from './api/problem';
import { idempotencyKey, rest, unwrap } from './api/rest';
import { addDays } from './inventory';
import { occupancyText } from './night-audit';

/** The Reports page: the headline figures with last year, each closed day and the breakdowns. */
export const ReportSummaryDocument = graphql(`
	query ReportSummary($propertyId: UUID!, $from: Date!, $to: Date!) {
		reportSummary(propertyId: $propertyId, from: $from, to: $to) {
			closedFrom
			closedTo
			notClosed
			headline {
				occupancyBp
				adr
				revpar
				roomNights
				totalRevenue
			}
			lastYear {
				occupancyBp
				adr
				revpar
				roomNights
				totalRevenue
			}
			days {
				businessDate
				roomsAvailable
				roomsSold
				occupancyBp
				adults
				children
				arrivals
				departures
				roomRevenue
				mealPlanRevenue
				posRevenue
				feeRevenue
				otherRevenue
				totalRevenue
				adr
				revpar
			}
			breakdowns {
				dimension
				keys {
					key
					roomNights
					roomRevenue
					revenue
				}
			}
		}
	}
`);

/** The Daily lists page. */
export const DailyListDocument = graphql(`
	query DailyList($propertyId: UUID!, $date: Date!, $kind: DailyListKind!) {
		dailyList(propertyId: $propertyId, date: $date, kind: $kind) {
			reservationRoomId
			reservationId
			confirmation
			guest
			roomNumber
			arrival
			departure
			status
			adults
			children
		}
	}
`);

/** SLTDA's Form A of a fiscal quarter. */
export const TdlReportDocument = graphql(`
	query TdlReport($propertyId: UUID!, $fyStartYear: Int!, $quarter: Int!) {
		tdlReport(propertyId: $propertyId, fyStartYear: $fyStartYear, quarter: $quarter) {
			name
			registrationNo
			periodLabel
			room
			food
			bar
			other
			totalTurnover
			serviceCharge
			vat
			base
			rateBp
			tdl
			creditExceedsSales
			notClosed
		}
	}
`);

/** The SSCL of a calendar quarter, month by month. */
export const SsclReportDocument = graphql(`
	query SsclReport($propertyId: UUID!, $year: Int!, $quarter: Int!) {
		ssclReport(propertyId: $propertyId, year: $year, quarter: $quarter) {
			months {
				month
				turnover
				exemptSameMonth
				exemptEarlierMonths
				badDebts
				liable
				levy
				excessExemption
				foreignCash
				due
				notClosed
			}
			quarterTotals {
				turnover
				exemptSameMonth
				exemptEarlierMonths
				badDebts
				liable
				levy
				foreignCash
			}
			returnDue
			sltdaRegistered
			exemptionNote
			methodNote
		}
	}
`);

/** A VAT return period, with its schedules. */
export const VatReportDocument = graphql(`
	query VatReport($propertyId: UUID!, $period: String!) {
		vatReport(propertyId: $propertyId, period: $period) {
			periodCode
			valueOfSupply
			outputVat
			creditNotesValue
			creditNotesVat
			netValue
			netVat
			schedule01 {
				serialNo
				invoiceDate
				invoiceNo
				purchaserTin
				purchaserName
				description
				valueOfSupply
				vatAmount
			}
			schedule04 {
				serialNo
				tin
				invoiceDate
				invoiceNo
				noteDate
				noteNo
				value
				vatAmount
			}
			timingDifference {
				rateBp
				value
				vat
			}
			notClosed
		}
	}
`);

/** A month's service charge. */
export const ServiceChargeReportDocument = graphql(`
	query ServiceChargeReport($propertyId: UUID!, $year: Int!, $month: Int!) {
		serviceChargeReport(propertyId: $propertyId, year: $year, month: $month) {
			byOutlet {
				outlet
				collected
				absorbed
			}
			rooms {
				collected
				absorbed
			}
			other {
				collected
				absorbed
			}
			total {
				collected
				absorbed
			}
			notClosed
		}
	}
`);

/** The signed-in user's report emails of the property. */
export const ReportEmailsDocument = graphql(`
	query ReportEmails($propertyId: UUID!) {
		reportEmails(propertyId: $propertyId) {
			kind
			enabled
		}
	}
`);

/** The report the close stored for a period, whose PDF is the record once it is ready. */
export const StoredReportDocument = graphql(`
	query StoredReport(
		$propertyId: UUID!
		$kind: StoredReportKind!
		$periodStart: Date!
		$periodEnd: Date!
	) {
		storedReport(
			propertyId: $propertyId
			kind: $kind
			periodStart: $periodStart
			periodEnd: $periodEnd
		) {
			id
			status
		}
	}
`);

export type ReportSummary = ReportSummaryQuery['reportSummary'];
export type ReportDay = ReportSummary['days'][number];
export type DailyListRow = DailyListQuery['dailyList'][number];
export type TdlReport = TdlReportQuery['tdlReport'];
export type SsclReport = SsclReportQuery['ssclReport'];
export type VatReport = VatReportQuery['vatReport'];
export type ServiceChargeReport = ServiceChargeReportQuery['serviceChargeReport'];
export type ReportEmail = ReportEmailsQuery['reportEmails'][number];
export type RenderReportRequest = components['schemas']['RenderReportRequest'];

/** The closed days only change when a day closes, so the reports are read again when the page is. */
export function reportKey(propertyId: string, ...rest: (string | number)[]) {
	return ['report', propertyId, ...rest] as const;
}

export function reportEmailsKey(propertyId: string) {
	return ['report-emails', propertyId] as const;
}

export async function fetchReportSummary(
	propertyId: string,
	from: string,
	to: string,
	signal?: AbortSignal
) {
	return (await query(ReportSummaryDocument, { propertyId, from, to }, signal)).reportSummary;
}

export async function fetchDailyList(
	propertyId: string,
	date: string,
	kind: DailyListKind,
	signal?: AbortSignal
) {
	return (await query(DailyListDocument, { propertyId, date, kind }, signal)).dailyList;
}

export async function fetchTdlReport(
	propertyId: string,
	fyStartYear: number,
	quarter: number,
	signal?: AbortSignal
) {
	return (await query(TdlReportDocument, { propertyId, fyStartYear, quarter }, signal)).tdlReport;
}

export async function fetchSsclReport(
	propertyId: string,
	year: number,
	quarter: number,
	signal?: AbortSignal
) {
	return (await query(SsclReportDocument, { propertyId, year, quarter }, signal)).ssclReport;
}

export async function fetchVatReport(propertyId: string, period: string, signal?: AbortSignal) {
	return (await query(VatReportDocument, { propertyId, period }, signal)).vatReport;
}

export async function fetchServiceChargeReport(
	propertyId: string,
	year: number,
	month: number,
	signal?: AbortSignal
) {
	return (await query(ServiceChargeReportDocument, { propertyId, year, month }, signal))
		.serviceChargeReport;
}

export async function fetchReportEmails(propertyId: string, signal?: AbortSignal) {
	return (await query(ReportEmailsDocument, { propertyId }, signal)).reportEmails;
}

/** Turns the signed-in user's own weekly or monthly email of the property on or off. */
export async function setReportEmail(
	propertyId: string,
	kind: 'weekly' | 'monthly',
	enabled: boolean
) {
	return unwrap(
		await rest.PUT('/api/v1/properties/{property}/report-emails/{kind}', {
			params: { path: { property: propertyId, kind } },
			body: { enabled }
		})
	);
}

/** Whole rupees: `Rs 3,453,513`. */
const RUPEES = new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 });

/** Whole rupees, as the VAT report holds them. */
export function formatRupees(rupees: number): string {
	return `${rupees < 0 ? '-' : ''}Rs ${RUPEES.format(Math.abs(rupees))}`;
}

/**
 * Base-currency minor units (cents), as the everyday reports, TDL, SSCL and service charge hold them, as whole
 * rupees. The printed liability figures are already whole-rupee multiples of 100.
 */
export function formatLkr(minor: number): string {
	return formatRupees(Math.round(minor / 100));
}

/** Basis points with one decimal: 7500 is `75.0%`. */
export function formatOccupancy(bp: number): string {
	return occupancyText(bp);
}

export interface Range {
	id: string;
	label: string;
	from: string;
	to: string;
}

/** The last day of the month a date is in. */
function monthEnd(date: string): string {
	const [year, month] = date.split('-').map(Number);
	return new Date(Date.UTC(year, month, 0)).toISOString().slice(0, 10);
}

/** The range picker's presets for the business date `today`; weeks run Monday to Sunday. */
export function presets(today: string): Range[] {
	const weekday = (new Date(`${today}T00:00:00Z`).getUTCDay() + 6) % 7;
	const monday = addDays(today, -weekday);
	const monthStart = `${today.slice(0, 7)}-01`;
	const lastMonthEnd = addDays(monthStart, -1);
	return [
		{ id: 'this-week', label: 'This week', from: monday, to: addDays(monday, 6) },
		{ id: 'last-week', label: 'Last week', from: addDays(monday, -7), to: addDays(monday, -1) },
		{ id: 'this-month', label: 'This month', from: monthStart, to: monthEnd(today) },
		{
			id: 'last-month',
			label: 'Last month',
			from: `${lastMonthEnd.slice(0, 7)}-01`,
			to: lastMonthEnd
		}
	];
}

const MONTHS = [
	'January',
	'February',
	'March',
	'April',
	'May',
	'June',
	'July',
	'August',
	'September',
	'October',
	'November',
	'December'
];

export function monthName(month: number): string {
	return MONTHS[month - 1];
}

/** A quarter of three months from `first` (1 to 12) of `year`, as `April – June (2026)`. */
function quarterLabel(year: number, first: number): string {
	return `${MONTHS[first - 1]} – ${MONTHS[first + 1]} (${year})`;
}

/** A fiscal quarter as SLTDA's form prints it, with the year its months fall in. */
export function fiscalQuarterLabel(fyStartYear: number, quarter: number): string {
	return quarter === 4
		? quarterLabel(fyStartYear + 1, 1)
		: quarterLabel(fyStartYear, 4 + (quarter - 1) * 3);
}

function yearMonth(date: string): { year: number; month: number } {
	const [year, month] = date.split('-').map(Number);
	return { year, month };
}

/** The `count` newest fiscal quarters up to the one `today` falls in. */
export function recentFiscalQuarters(today: string, count: number) {
	const { year, month } = yearMonth(today);
	let fyStartYear = month >= 4 ? year : year - 1;
	let quarter = Math.floor(((month + 8) % 12) / 3) + 1;
	const quarters = [];
	for (let index = 0; index < count; index++) {
		quarters.push({ fyStartYear, quarter, label: fiscalQuarterLabel(fyStartYear, quarter) });
		if (quarter === 1) {
			fyStartYear -= 1;
			quarter = 4;
		} else {
			quarter -= 1;
		}
	}
	return quarters;
}

/** The `count` newest calendar quarters up to the one `today` falls in. */
export function recentQuarters(today: string, count: number) {
	const { year: currentYear, month } = yearMonth(today);
	let year = currentYear;
	let quarter = Math.ceil(month / 3);
	const quarters = [];
	for (let index = 0; index < count; index++) {
		quarters.push({ year, quarter, label: quarterLabel(year, (quarter - 1) * 3 + 1) });
		if (quarter === 1) {
			year -= 1;
			quarter = 4;
		} else {
			quarter -= 1;
		}
	}
	return quarters;
}

/** The `count` newest months up to the one `today` falls in. */
export function recentMonths(today: string, count: number) {
	let { year, month } = yearMonth(today);
	const months = [];
	for (let index = 0; index < count; index++) {
		months.push({ year, month, label: `${monthName(month)} ${year}` });
		if (month === 1) {
			year -= 1;
			month = 12;
		} else {
			month -= 1;
		}
	}
	return months;
}

/**
 * The `count` newest VAT return periods up to the one `today` falls in, by IRD's period code: `YYQ0` for a
 * quarter, `YYQM` for a month (counted within its quarter).
 */
export function recentVatPeriods(
	today: string,
	returnPeriod: 'monthly' | 'quarterly',
	count: number
) {
	if (returnPeriod === 'quarterly') {
		return recentQuarters(today, count).map(({ year, quarter, label }) => ({
			code: `${String(year % 100).padStart(2, '0')}${quarter}0`,
			label
		}));
	}
	return recentMonths(today, count).map(({ year, month, label }) => ({
		code: `${String(year % 100).padStart(2, '0')}${Math.ceil(month / 3)}${((month - 1) % 3) + 1}`,
		label
	}));
}

/** The index a tab list moves to on `key` (arrows wrap; Home and End jump), or null for any other key. */
export function tabTarget(key: string, index: number, count: number): number | null {
	if (key === 'ArrowRight') return (index + 1) % count;
	if (key === 'ArrowLeft') return (index - 1 + count) % count;
	if (key === 'Home') return 0;
	if (key === 'End') return count - 1;
	return null;
}

const POLL_MS = 1000;
const POLLS = 30;

/** Resolves after `ms`, or rejects with the abort reason as soon as `signal` aborts. */
function sleep(ms: number, signal?: AbortSignal): Promise<void> {
	return new Promise((done, fail) => {
		if (signal?.aborted) return fail(signal.reason);
		const timer = setTimeout(done, ms);
		signal?.addEventListener(
			'abort',
			() => {
				clearTimeout(timer);
				fail(signal.reason);
			},
			{ once: true }
		);
	});
}

/** Renders a report as a PDF and returns a link to it, waiting up to 30 seconds for the render. */
export async function reportLink(
	propertyId: string,
	request: RenderReportRequest,
	signal?: AbortSignal
): Promise<string> {
	const { id } = unwrap(
		await rest.POST('/api/v1/properties/{property}/reports/render', {
			params: { path: { property: propertyId }, header: { 'Idempotency-Key': idempotencyKey() } },
			body: request,
			signal
		})
	);
	for (let poll = 0; poll < POLLS; poll++) {
		const read = unwrap(
			await rest.GET('/api/v1/properties/{property}/reports/{report}', {
				params: { path: { property: propertyId, report: id } },
				signal
			})
		);
		if (read.status === 'ready' && read.url) return read.url;
		if (read.status === 'failed') {
			throw new ApiError({
				type: 'about:blank',
				title: 'The report could not be rendered',
				status: 500
			});
		}
		await sleep(POLL_MS, signal);
	}
	throw new ApiError({
		type: 'about:blank',
		title: 'The report is taking too long to render. Try again in a moment.',
		status: 504
	});
}

/** A period the close stores a report of. */
export interface StoredPeriod {
	kind: StoredReportKind;
	periodStart: string;
	periodEnd: string;
}

/** The first day of month `month` (1 to 12) of `year`. */
function monthStart(year: number, month: number): string {
	return `${year}-${String(month).padStart(2, '0')}-01`;
}

/** Months `first` to `first + count - 1` of `year` as a period of `kind`. */
function monthsPeriod(kind: StoredReportKind, year: number, first: number, count: number) {
	return {
		kind,
		periodStart: monthStart(year, first),
		periodEnd: monthEnd(monthStart(year, first + count - 1))
	};
}

/**
 * The period of the report the close stores that `request` asks for: every liability period, and a summary of a
 * whole Monday-to-Sunday week or calendar month. `null` for anything else, which is only ever rendered on demand.
 */
export function storedPeriod(request: RenderReportRequest): StoredPeriod | null {
	switch (request.report) {
		case 'summary': {
			const { from, to } = request.params;
			const monday = new Date(`${from}T00:00:00Z`).getUTCDay() === 1;
			if (monday && to === addDays(from, 6)) {
				return { kind: 'WEEKLY', periodStart: from, periodEnd: to };
			}
			if (from.endsWith('-01') && to === monthEnd(from)) {
				return { kind: 'MONTHLY', periodStart: from, periodEnd: to };
			}
			return null;
		}
		case 'tdl': {
			const { fy_start_year: year, quarter } = request.params;
			return quarter === 4
				? monthsPeriod('TDL', year + 1, 1, 3)
				: monthsPeriod('TDL', year, 4 + (quarter - 1) * 3, 3);
		}
		case 'sscl':
			return monthsPeriod('SSCL', request.params.year, (request.params.quarter - 1) * 3 + 1, 3);
		case 'vat': {
			const [y1, y2, quarter, month] = request.params.period.split('').map(Number);
			const year = 2000 + y1 * 10 + y2;
			return month === 0
				? monthsPeriod('VAT', year, (quarter - 1) * 3 + 1, 3)
				: monthsPeriod('VAT', year, (quarter - 1) * 3 + month, 1);
		}
		case 'service_charge':
			return monthsPeriod('SERVICE_CHARGE', request.params.year, request.params.month, 1);
		case 'daily_list':
			return null;
	}
}

/**
 * A link to the PDF of `request`'s report: the one the close stored for the period once it is ready, as that is the
 * record, or else one rendered now (`reportLink`).
 */
export async function reportPdfLink(
	propertyId: string,
	request: RenderReportRequest,
	signal?: AbortSignal
): Promise<string> {
	const period = storedPeriod(request);
	if (period) {
		const { storedReport } = await query(StoredReportDocument, { propertyId, ...period }, signal);
		if (storedReport?.status === 'READY') {
			const read = unwrap(
				await rest.GET('/api/v1/properties/{property}/reports/{report}', {
					params: { path: { property: propertyId, report: storedReport.id } },
					signal
				})
			);
			if (read.status === 'ready' && read.url) return read.url;
		}
	}
	return reportLink(propertyId, request, signal);
}

/** Saves a VAT schedule's CSV under the file name the server gives it. */
export async function downloadSchedule(
	propertyId: string,
	schedule: '01' | '04',
	period: string
): Promise<void> {
	const params = { path: { property: propertyId, period } };
	const result =
		schedule === '01'
			? await rest.GET('/api/v1/properties/{property}/reports/vat/{period}/schedule01.csv', {
					params,
					parseAs: 'blob'
				})
			: await rest.GET('/api/v1/properties/{property}/reports/vat/{period}/schedule04.csv', {
					params,
					parseAs: 'blob'
				});
	const blob = unwrap(result);
	const disposition = result.response.headers.get('content-disposition') ?? '';
	const name =
		/filename="?([^";]+)"?/.exec(disposition)?.[1] ?? `schedule${schedule}_${period}.csv`;
	const url = URL.createObjectURL(blob);
	const anchor = document.createElement('a');
	anchor.href = url;
	anchor.download = name;
	anchor.click();
	setTimeout(() => URL.revokeObjectURL(url));
}

const DIMENSION_LABELS = {
	SEGMENT: 'Segment',
	SOURCE: 'Source',
	RESIDENCY: 'Residency',
	ROOM_TYPE: 'Room type',
	OUTLET: 'Outlet'
} as const;

export function dimensionLabel(dimension: keyof typeof DIMENSION_LABELS): string {
	return DIMENSION_LABELS[dimension];
}

/** A breakdown key as a person reads it: `room_type` is not, `unattributed` is Unattributed. */
export function keyLabel(key: string): string {
	const text = key.replace(/_/g, ' ');
	return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The dates of `from` to `to`, both included. */
export function datesBetween(from: string, to: string): string[] {
	const dates = [];
	for (let date = from; date <= to; date = addDays(date, 1)) dates.push(date);
	return dates;
}
