import { describe, expect, it } from 'vitest';
import {
	accountKindLabel,
	accountsKey,
	contactSummary,
	formatCreditLimit,
	type Account
} from './accounts';

const contact = (overrides: Partial<Account['contact']> = {}): Account['contact'] => ({
	email: null,
	phone: null,
	address: null,
	contactName: null,
	...overrides
});

describe('accountsKey', () => {
	it("matches the server's accounts:<property> event", () => {
		expect(accountsKey('p1')).toEqual(['accounts:p1', '', false]);
	});

	it('keeps the property-scoped prefix regardless of search or includeInactive', () => {
		expect(accountsKey('p1', 'acme', true)).toEqual(['accounts:p1', 'acme', true]);
		expect(accountsKey('p1')[0]).toBe(accountsKey('p1', 'acme', true)[0]);
	});
});

describe('accountKindLabel', () => {
	it('labels each kind', () => {
		expect(accountKindLabel('COMPANY')).toBe('Company');
		expect(accountKindLabel('TRAVEL_AGENT')).toBe('Travel agent');
	});
});

describe('formatCreditLimit', () => {
	it('formats an amount in the currency', () => {
		expect(formatCreditLimit(500000, 'USD')).toBe('5,000.00');
	});

	it('shows "No limit" when none is set', () => {
		expect(formatCreditLimit(null, 'USD')).toBe('No limit');
		expect(formatCreditLimit(undefined, 'USD')).toBe('No limit');
	});
});

describe('contactSummary', () => {
	it('joins the parts that are on file', () => {
		expect(
			contactSummary(contact({ contactName: 'Priya', email: 'priya@acme.test', phone: '077' }))
		).toBe('Priya · priya@acme.test · 077');
		expect(contactSummary(contact({ email: 'priya@acme.test' }))).toBe('priya@acme.test');
	});

	it('is empty when nothing is on file', () => {
		expect(contactSummary(contact())).toBe('');
	});
});
