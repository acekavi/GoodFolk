import { describe, expect, it } from 'vitest';
import { safeRedirect } from './redirect';

const origin = 'https://pms.example';

describe('safeRedirect', () => {
	it('keeps a path on this origin, with its query', () => {
		expect(safeRedirect('/hk?property=abc', origin)).toBe('/hk?property=abc');
	});

	it.each([
		null,
		'',
		'hk',
		'//evil.example',
		'/\\evil.example',
		'https://evil.example/hk',
		'javascript:alert(1)'
	])('refuses %s', (raw) => {
		expect(safeRedirect(raw, origin)).toBeNull();
	});
});
