import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/** The headers `static/_headers` gives the paths matching `pattern` (Cloudflare Pages' format). */
function headersFor(pattern: string): Record<string, string> {
	const headers: Record<string, string> = {};
	let current: string | null = null;
	for (const line of readFileSync('static/_headers', 'utf8').split('\n')) {
		if (line.startsWith('#') || line.trim() === '') continue;
		if (!/^\s/.test(line)) {
			current = line.trim();
			continue;
		}
		if (current !== pattern) continue;
		const [name, value] = line.trim().split(/:\s*(.*)/s, 2);
		headers[name.toLowerCase()] = value;
	}
	return headers;
}

describe('static/_headers', () => {
	it('forbids framing and sniffing on every page, and sends only the origin as a referrer', () => {
		expect(headersFor('/*')).toEqual({
			'content-security-policy': "frame-ancestors 'none'",
			'x-frame-options': 'DENY',
			'x-content-type-options': 'nosniff',
			'referrer-policy': 'strict-origin-when-cross-origin'
		});
	});
});
