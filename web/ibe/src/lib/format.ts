/**
 * ISO 4217 minor units, as the server's `fx::exponent`: 0, 3 for thousandths, else 2. Every amount the public API
 * sends is in minor units of its currency.
 */
export function exponent(currency: string): number {
	if (
		[
			'BIF',
			'CLP',
			'DJF',
			'GNF',
			'ISK',
			'JPY',
			'KMF',
			'KRW',
			'PYG',
			'RWF',
			'UGX',
			'VND',
			'VUV',
			'XAF',
			'XOF',
			'XPF'
		].includes(currency)
	)
		return 0;
	if (['BHD', 'IQD', 'JOD', 'KWD', 'LYD', 'OMR', 'TND'].includes(currency)) return 3;
	return 2;
}

/** The symbol guests know a currency by; any other currency is shown by its code. */
const SYMBOLS: Record<string, string> = { LKR: 'Rs', USD: 'US$' };

/**
 * An amount for a guest: `1234500` LKR is `Rs 12,345`, `12345` USD is `US$ 123.45`, `12345` EUR is `EUR 123.45`.
 * Digits come from exact integer arithmetic, never rounded. Rupee prices are whole rupees, so LKR drops its cents
 * when they are zero; any other amount shows all of its currency's decimals.
 */
export function money(minor: number, currency: string): string {
	const digits = exponent(currency);
	const sign = minor < 0 ? '-' : '';
	const text = String(Math.abs(minor)).padStart(digits + 1, '0');
	const whole = text.slice(0, text.length - digits).replace(/\B(?=(\d{3})+$)/g, ',');
	const fraction = text.slice(text.length - digits);
	const shown =
		digits === 0 || (currency === 'LKR' && /^0+$/.test(fraction)) ? whole : `${whole}.${fraction}`;
	return `${sign}${SYMBOLS[currency] ?? currency} ${shown}`;
}

/** Totals per currency, as a booking's summary lists them: `US$ 240.00 + Rs 30,000`. */
export function totalsText(totals: Record<string, number>): string {
	return Object.entries(totals)
		.map(([currency, minor]) => money(minor, currency))
		.join(' + ');
}

/** `2026-11-03` as its parts, for dates that are calendar days at the property, never instants. */
function utc(date: string): number {
	const [year, month, day] = date.split('-').map(Number);
	return Date.UTC(year, month - 1, day);
}

/**
 * Stay dates are the property's calendar days (the API sends them in its time zone), so they are formatted in UTC:
 * no visitor's own time zone can move one to the day before.
 */
const LONG_DATE = new Intl.DateTimeFormat('en-GB', {
	timeZone: 'UTC',
	weekday: 'short',
	day: 'numeric',
	month: 'short',
	year: 'numeric'
});
const SHORT_DATE = new Intl.DateTimeFormat('en-GB', {
	timeZone: 'UTC',
	weekday: 'short',
	day: 'numeric',
	month: 'short'
});

/** `2026-11-03` as `Tue, 3 Nov 2026`. */
export function dateText(date: string): string {
	return LONG_DATE.format(utc(date));
}

/** `2026-11-03` as `Tue, 3 Nov`, for a night in a breakdown. */
export function shortDateText(date: string): string {
	return SHORT_DATE.format(utc(date));
}

/** The nights between two calendar dates. */
export function nights(checkIn: string, checkOut: string): number {
	return Math.round((utc(checkOut) - utc(checkIn)) / 86_400_000);
}

/** `date` moved by `days` calendar days. */
export function addDays(date: string, days: number): string {
	return new Date(utc(date) + days * 86_400_000).toISOString().slice(0, 10);
}

/** `date` moved by `months`, on the same day or the month's last when it is shorter, as the server counts. */
export function addMonths(date: string, months: number): string {
	const [year, month, day] = date.split('-').map(Number);
	const index = month - 1 + months;
	const target = new Date(Date.UTC(year + Math.floor(index / 12), index % 12, 1));
	const last = new Date(
		Date.UTC(target.getUTCFullYear(), target.getUTCMonth() + 1, 0)
	).getUTCDate();
	target.setUTCDate(Math.min(day, last));
	return target.toISOString().slice(0, 10);
}

export function plural(count: number, one: string, many = `${one}s`): string {
	return `${count} ${count === 1 ? one : many}`;
}

/** `2 adults, 1 child`. */
export function guestsText(adults: number, children: number): string {
	const parts = [plural(adults, 'adult')];
	if (children > 0) parts.push(plural(children, 'child', 'children'));
	return parts.join(', ');
}

const MEAL_PLANS: Record<string, string> = {
	RO: 'Room only',
	BB: 'Bed and breakfast',
	HB: 'Half board',
	FB: 'Full board'
};

/** A meal plan code as guests read it. */
export function mealPlanText(code: string): string {
	return MEAL_PLANS[code] ?? code;
}

/** Beds as guests read them: `1 king bed, 2 single beds`. */
export function bedsText(beds: readonly { kind: string; count: number }[]): string {
	return beds.map((bed) => plural(bed.count, `${bed.kind.replaceAll('_', ' ')} bed`)).join(', ');
}
