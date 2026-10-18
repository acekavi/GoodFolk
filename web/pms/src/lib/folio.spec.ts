import { describe, expect, it } from 'vitest';
import {
	balanceText,
	canCorrect,
	categories,
	exponent,
	folioActions,
	folioLabels,
	formatAmount,
	invoiceBlocker,
	isSettled,
	money,
	parseAmount,
	paymentDetails,
	ratePercent,
	refundable,
	reversedLines,
	transferCounterpart,
	uncredited,
	type Folio,
	type FolioLine,
	type Payment
} from './folio';

function line(id: string, extra: Partial<FolioLine> = {}): FolioLine {
	return {
		id,
		source: 'MANUAL',
		category: 'misc',
		description: 'Minibar',
		quantity: 1,
		net: 1000,
		total: 1100,
		currency: 'USD',
		baseTotal: 330000,
		businessDate: '2026-10-18',
		reverses: null,
		reason: null,
		transferId: null,
		postedAt: '2026-10-18T10:00:00Z',
		taxes: [],
		...extra
	};
}

function payment(id: string, extra: Partial<Payment> = {}): Payment {
	return {
		id,
		method: 'cash',
		amount: 1100,
		currency: 'USD',
		baseAmount: 330000,
		details: {},
		evidence: null,
		refundOf: null,
		businessDate: '2026-10-18',
		transferId: null,
		takenAt: '2026-10-18T10:00:00Z',
		...extra
	};
}

function folio(id: string, extra: Partial<Folio> = {}): Folio {
	return {
		id,
		kind: 'GUEST',
		status: 'OPEN',
		reservationRoomId: null,
		reservationId: null,
		accountId: null,
		version: 1,
		linesOmitted: false,
		truncated: false,
		lines: [],
		payments: [],
		balances: [],
		invoices: [],
		...extra
	};
}

const ALL = { post: true, adjust: true, invoice: true };

describe('money', () => {
	it("uses each currency's exponent", () => {
		expect(exponent('USD')).toBe(2);
		expect(exponent('LKR')).toBe(2);
		expect(exponent('JPY')).toBe(0);
		for (const currency of ['KWD', 'BHD', 'OMR']) expect(exponent(currency)).toBe(3);
	});

	it('formats minor units with separators and a leading minus', () => {
		expect(formatAmount(123450, 'USD')).toBe('1,234.50');
		expect(formatAmount(-123450, 'USD')).toBe('-1,234.50');
		expect(formatAmount(5, 'USD')).toBe('0.05');
		expect(formatAmount(-5, 'LKR')).toBe('-0.05');
		expect(formatAmount(0, 'USD')).toBe('0.00');
		expect(formatAmount(1234567, 'JPY')).toBe('1,234,567');
		expect(formatAmount(1234567, 'KWD')).toBe('1,234.567');
		expect(formatAmount(100, 'OMR')).toBe('0.100');
		expect(formatAmount(900719925474099, 'LKR')).toBe('9,007,199,254,740.99');
		expect(money(30000000, 'LKR')).toBe('LKR 300,000.00');
	});

	it('parses typed amounts exactly, never through floats', () => {
		expect(parseAmount('1,234.50', 'USD')).toBe(123450);
		expect(parseAmount(' 0.1 ', 'USD')).toBe(10);
		expect(parseAmount('0.29', 'USD')).toBe(29);
		expect(parseAmount('1.15', 'USD')).toBe(115);
		expect(parseAmount('10', 'USD')).toBe(1000);
		expect(parseAmount('10.', 'USD')).toBe(1000);
		expect(parseAmount('1500', 'JPY')).toBe(1500);
		expect(parseAmount('1.005', 'KWD')).toBe(1005);
		expect(parseAmount('0', 'USD')).toBe(0);
		expect(parseAmount('12,345,678', 'USD')).toBe(1234567800);
		expect(parseAmount('90,071,992,547,409.91', 'USD')).toBe(9007199254740991);
	});

	it('refuses what is not an amount in the currency', () => {
		for (const text of [
			'',
			'abc',
			'-5',
			'1.234',
			'1e3',
			'.5',
			'1.2.3',
			'9007199254740993',
			'90,071,992,547,409.92',
			'12,50',
			'1,2,3',
			',5',
			'1,23.45',
			'1234,567'
		]) {
			expect(parseAmount(text, 'USD'), text).toBeNull();
		}
		expect(parseAmount('1.5', 'JPY')).toBeNull();
	});
});

describe('balances', () => {
	it('shows the base equivalent of a foreign currency only', () => {
		expect(balanceText({ currency: 'USD', amount: 1000, baseAmount: 300000 }, 'LKR')).toBe(
			'USD 10.00 (LKR 3,000.00)'
		);
		expect(balanceText({ currency: 'LKR', amount: -500, baseAmount: -500 }, 'LKR')).toBe(
			'LKR -5.00'
		);
	});

	it('is settled when every currency is at zero', () => {
		expect(isSettled(folio('f'))).toBe(true);
		const zero = { currency: 'USD', amount: 0, baseAmount: 0 };
		expect(isSettled(folio('f', { balances: [zero] }))).toBe(true);
		expect(
			isSettled(folio('f', { balances: [zero, { currency: 'LKR', amount: 1, baseAmount: 1 }] }))
		).toBe(false);
	});
});

describe('issuing an invoice', () => {
	const settled = { currency: 'USD', amount: 0, baseAmount: 0 };

	it('is possible only with lines and a zero balance', () => {
		expect(invoiceBlocker(folio('f', { lines: [line('l')], balances: [settled] }))).toBeNull();
		expect(invoiceBlocker(folio('f'))).toBe('Nothing to invoice: the folio has no charges.');
		expect(
			invoiceBlocker(
				folio('f', {
					lines: [line('l')],
					balances: [
						{ currency: 'USD', amount: 1100, baseAmount: 330000 },
						{ currency: 'LKR', amount: -500, baseAmount: -500 }
					]
				})
			)
		).toBe('Settle the balance first: USD 11.00 due, LKR 5.00 in credit.');
	});

	it('is refused for an invoiced folio and for an account ledger read without its lines', () => {
		expect(invoiceBlocker(folio('f', { status: 'INVOICED', lines: [line('l')] }))).toMatch(
			/credit note/
		);
		expect(invoiceBlocker(folio('f', { kind: 'ACCOUNT', linesOmitted: true }))).toMatch(
			/Open the account/
		);
	});

	it('offers a credit note for invoices not yet credited', () => {
		const invoice = {
			id: 'i1',
			kind: 'INVOICE' as const,
			serial: 'GAL-2026-000001',
			totalLkr: 100,
			issuedAt: '2026-10-18T10:00:00Z',
			credits: null,
			pdfReady: true
		};
		const note = { ...invoice, id: 'c1', kind: 'CREDIT_NOTE' as const, credits: 'i1' };
		const again = { ...invoice, id: 'i2' };
		expect([...uncredited(folio('f', { invoices: [invoice] }))]).toEqual(['i1']);
		expect([...uncredited(folio('f', { invoices: [invoice, note, again] }))]).toEqual(['i2']);
	});
});

describe('folio actions', () => {
	it('follow the permissions', () => {
		const open = folio('f');
		expect(folioActions(open, ALL)).toEqual({
			postCharge: true,
			takePayment: true,
			correct: true,
			refund: true,
			issueInvoice: true,
			viewInvoice: true,
			creditNote: true
		});
		expect(folioActions(open, { post: true, adjust: false, invoice: true })).toEqual({
			postCharge: true,
			takePayment: true,
			correct: false,
			refund: false,
			issueInvoice: true,
			viewInvoice: true,
			creditNote: false
		});
		expect(folioActions(open, { post: false, adjust: false, invoice: true })).toMatchObject({
			postCharge: false,
			takePayment: false,
			issueInvoice: true,
			viewInvoice: true
		});
		expect(folioActions(open, { post: false, adjust: false, invoice: false })).toMatchObject({
			issueInvoice: false,
			viewInvoice: false
		});
	});

	it('change nothing on an invoiced folio or an account ledger read without lines', () => {
		const invoiced = folioActions(folio('f', { status: 'INVOICED' }), ALL);
		expect(invoiced).toMatchObject({
			postCharge: false,
			takePayment: false,
			correct: false,
			refund: false
		});
		expect(invoiced.creditNote).toBe(true);
		const ledger = folioActions(folio('f', { kind: 'ACCOUNT', linesOmitted: true }), ALL);
		expect(ledger).toMatchObject({ postCharge: false, correct: false, issueInvoice: false });
		expect(ledger.viewInvoice).toBe(true);
	});
});

describe('lines', () => {
	it('can be corrected once, and a reversal never', () => {
		const original = line('l1');
		const reversal = line('l2', { reverses: 'l1', total: -1100 });
		const other = line('l3');
		const reversed = reversedLines([original, reversal, other]);
		expect(canCorrect(original, reversed)).toBe(false);
		expect(canCorrect(reversal, reversed)).toBe(false);
		expect(canCorrect(other, reversed)).toBe(true);
	});

	it('name the other folio of a transfer', () => {
		const out = line('l2', { reverses: 'l1', transferId: 't' });
		const moved = line('l3', { transferId: 't' });
		const folios = [
			folio('a', { lines: [line('l1'), out] }),
			folio('b', { lines: [moved] }),
			folio('c', { lines: [line('l4')] })
		];
		expect(transferCounterpart(out, 'a', folios)).toEqual({ direction: 'to', folioId: 'b' });
		expect(transferCounterpart(moved, 'b', folios)).toEqual({ direction: 'from', folioId: 'a' });
		expect(transferCounterpart(line('l4'), 'c', folios)).toBeNull();
	});
});

describe('folio labels', () => {
	it('name the deposit, each stay by room or type, and the account', () => {
		const folios = [
			folio('d', { kind: 'DEPOSIT' }),
			folio('g1', { reservationRoomId: 'rr1' }),
			folio('g2', { reservationRoomId: 'rr2' }),
			folio('g3', { reservationRoomId: 'rr3' }),
			folio('a', { kind: 'ACCOUNT' })
		];
		const rooms = [
			{ id: 'rr1', room: { number: '101' }, roomType: { code: 'DLX' } },
			{ id: 'rr2', room: null, roomType: { code: 'DLX' } },
			{ id: 'rr3', room: null, roomType: { code: 'DLX' } }
		];
		expect([...folioLabels(folios, rooms, 'Acme').values()]).toEqual([
			'Deposit',
			'Room 101',
			'DLX · unassigned',
			'DLX · unassigned 2',
			'Account · Acme'
		]);
	});
});

describe('payments', () => {
	it('can be refunded down to what is left, wherever the refunds were made', () => {
		const paid = payment('p1', { amount: 5000 });
		const folios = [
			folio('d', { payments: [paid, payment('p2', { amount: -1000, refundOf: 'p1' })] }),
			folio('g', { payments: [payment('p3', { amount: -1500, refundOf: 'p1' })] })
		];
		expect(refundable(paid, folios)).toBe(2500);
		expect(refundable(payment('p4', { method: 'account' }), folios)).toBe(0);
		expect(refundable(payment('p5', { method: 'transfer' }), folios)).toBe(0);
		expect(refundable(payment('p6', { amount: -100 }), folios)).toBe(0);
	});

	it('show their details on one line', () => {
		const labels = new Map([['g', 'Room 101']]);
		expect(
			paymentDetails(
				payment('p', { details: { brand: 'Visa', last4: '4242', approval: 'A1' } }),
				labels
			)
		).toBe('Visa ····4242 · approval A1');
		expect(paymentDetails(payment('p', { details: { reference: 'TT-1' } }), labels)).toBe(
			'ref. TT-1'
		);
		expect(paymentDetails(payment('p', { details: { to_folio: 'g' } }), labels)).toBe(
			'to Room 101'
		);
		expect(paymentDetails(payment('p', { details: null }), labels)).toBe('');
	});
});

describe('categories', () => {
	it('list the fixed ones and each active outlet', () => {
		const outlets = [
			{ code: 'BAR', name: 'Bar', active: true },
			{ code: 'OLD', name: 'Old', active: false }
		];
		expect(categories(outlets).map((entry) => entry.value)).toEqual([
			'room',
			'meal_plan',
			'laundry',
			'misc',
			'fee',
			'pos:BAR'
		]);
		expect(ratePercent(1000)).toBe('10%');
		expect(ratePercent(250)).toBe('2.5%');
	});
});
