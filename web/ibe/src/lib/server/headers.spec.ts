import { describe, expect, it } from 'vitest';
import { withSecurityHeaders } from './headers';

describe('withSecurityHeaders', () => {
	it('forbids framing and sniffing, and sends only the origin as a referrer', () => {
		const headers = new Headers({ 'content-type': 'text/html' });

		withSecurityHeaders(headers);

		expect(headers.get('content-security-policy')).toBe("frame-ancestors 'none'");
		expect(headers.get('x-frame-options')).toBe('DENY');
		expect(headers.get('x-content-type-options')).toBe('nosniff');
		expect(headers.get('referrer-policy')).toBe('strict-origin-when-cross-origin');
		expect(headers.get('content-type')).toBe('text/html');
	});

	it("keeps a page's own referrer policy", () => {
		const headers = new Headers({ 'referrer-policy': 'no-referrer' });

		withSecurityHeaders(headers);

		expect(headers.get('referrer-policy')).toBe('no-referrer');
	});
});
