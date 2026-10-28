import { describe, expect, it } from 'vitest';
import { ACCENT_TOO_PALE, accentProblem, contrastWithWhite, previewUrl } from './ibe';

describe('contrastWithWhite', () => {
	it('follows WCAG relative luminance', () => {
		expect(contrastWithWhite('#ffffff')).toBeCloseTo(1, 6);
		expect(contrastWithWhite('#000000')).toBeCloseTo(21, 6);
		expect(contrastWithWhite('#1d4ed8')).toBeGreaterThan(6.5);
		expect(contrastWithWhite('#1D4ED8')).toBe(contrastWithWhite('#1d4ed8'));
	});

	it('reads only #rrggbb', () => {
		for (const text of ['', 'ffffff', '#fff', '#12345', '#1234567', '#gggggg', 'red']) {
			expect(contrastWithWhite(text), text).toBeNull();
		}
	});
});

describe('accentProblem', () => {
	it('refuses a colour white text cannot be read on, and anything that is not a colour', () => {
		expect(accentProblem('#ffffff')).toBe(ACCENT_TOO_PALE);
		expect(accentProblem('#ffeb3b')).toBe(ACCENT_TOO_PALE);
		expect(accentProblem('blue')).toMatch(/colour like/);
	});

	it('takes the edge of 4.5 and darker colours', () => {
		expect(accentProblem('#1d4ed8')).toBeNull();
		expect(accentProblem('#767676')).toBeNull();
		expect(accentProblem('#777777')).toBe(ACCENT_TOO_PALE);
	});
});

describe('previewUrl', () => {
	it('joins the property code onto the public origin', () => {
		expect(previewUrl('GAL', 'https://book.example')).toBe('https://book.example/GAL');
		expect(previewUrl('GAL', 'https://book.example/')).toBe('https://book.example/GAL');
	});

	it('is absent when the build knows no origin', () => {
		expect(previewUrl('GAL', '')).toBeNull();
	});
});
