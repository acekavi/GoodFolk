import { describe, expect, it } from 'vitest';
import { toApiError } from './problem';

describe('toApiError', () => {
	it('uses the problem detail as the message', () => {
		const error = toApiError(
			{ type: 'about:blank', title: 'Conflict', status: 409, detail: 'code taken' },
			409
		);

		expect(error.message).toBe('code taken');
		expect(error.status).toBe(409);
	});

	it('falls back to a generic problem for non-problem bodies', () => {
		const error = toApiError('<html>bad gateway</html>', 502);

		expect(error.message).toBe('Request failed');
		expect(error.status).toBe(502);
	});
});
