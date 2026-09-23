import { describe, expect, it } from 'vitest';
import { errorMessage, NETWORK_ERROR, toApiError } from './problem';

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

describe('errorMessage', () => {
	it('shows an API error as its message', () => {
		expect(
			errorMessage(toApiError({ type: 'about:blank', title: 'Forbidden', status: 403 }, 403))
		).toBe('Forbidden');
	});

	it('shows anything else, such as a failed fetch, as a network error', () => {
		expect(errorMessage(new TypeError('Failed to fetch'))).toBe(NETWORK_ERROR);
	});
});
