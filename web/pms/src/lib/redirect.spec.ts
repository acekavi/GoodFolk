import { describe, expect, it } from 'vitest';
import { safeRedirect } from './redirect';

const origin = 'https://pms.example';

describe('safeRedirect', () => {
	it('keeps a path on this origin, with its query', () => {
		expect(safeRedirect('/hk?property=abc', origin)).toBe('/hk?property=abc');
	});

	it('keeps a // that is only in the query', () => {
		expect(safeRedirect('/hk?x=//evil', origin)).toBe('/hk?x=//evil');
	});

	it.each([
		null,
		'',
		'hk',
		'//evil.example',
		'/\\evil.example',
		'/\\evil.com',
		'https://evil.example/hk',
		'https://evil.com',
		'javascript:alert(1)',
		'//evil.com',
		'/.//evil.com',
		'/..//evil.com',
		'/%2E//evil.com',
		'/%2e%2e//evil.com',
		'/\t/evil.com',
		'/\n/evil.com',
		'/\r/evil.com',
		'/.\t/\t/evil.com',
		'/\t\\evil.com'
	])('refuses %s', (raw) => {
		expect(safeRedirect(raw, origin)).toBeNull();
	});
});
