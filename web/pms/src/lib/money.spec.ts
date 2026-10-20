import { describe, expect, it } from 'vitest';
import { exponent } from './money';

describe('exponent', () => {
	it("uses each currency's exponent", () => {
		expect(exponent('USD')).toBe(2);
		expect(exponent('LKR')).toBe(2);
		expect(exponent('JPY')).toBe(0);
		for (const currency of ['KWD', 'BHD', 'OMR']) expect(exponent(currency)).toBe(3);
	});
});
