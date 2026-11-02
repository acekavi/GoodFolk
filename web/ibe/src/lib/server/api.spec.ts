import { describe, expect, it } from 'vitest';
import { toApi } from './api';

const PUBLIC = 'https://api.example.lk';
const PRIVATE = 'https://core-api.internal';
const SECRET = 'an-ssr-secret-of-at-least-32-characters';

describe('a server render’s API call', () => {
	it('goes to the private origin naming the guest, with the SSR secret', () => {
		const sent = toApi(
			new Request(`${PUBLIC}/api/public/v1/ibe/GAL/search?rooms=2-0`),
			PUBLIC,
			PRIVATE,
			'203.0.113.7',
			SECRET
		);

		expect(sent.url).toBe(`${PRIVATE}/api/public/v1/ibe/GAL/search?rooms=2-0`);
		expect(sent.headers.get('x-ibe-client-ip')).toBe('203.0.113.7');
		expect(sent.headers.get('x-ibe-ssr-secret')).toBe(SECRET);
	});

	it('names no guest without a secret, as core-api would ignore it', () => {
		const sent = toApi(
			new Request(`${PUBLIC}/api/public/v1/ibe/GAL/site`),
			PUBLIC,
			PRIVATE,
			'203.0.113.7',
			undefined
		);

		expect(sent.url).toBe(`${PRIVATE}/api/public/v1/ibe/GAL/site`);
		expect(sent.headers.has('x-ibe-client-ip')).toBe(false);
		expect(sent.headers.has('x-ibe-ssr-secret')).toBe(false);
	});

	it('never sends the secret anywhere but the API', () => {
		const elsewhere = new Request('https://cdn.example.lk/photo.webp');

		const sent = toApi(elsewhere, PUBLIC, PRIVATE, '203.0.113.7', SECRET);

		expect(sent).toBe(elsewhere);
		expect(sent.headers.has('x-ibe-ssr-secret')).toBe(false);
	});
});
